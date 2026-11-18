use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::Workspace;
use crate::constants::{DEFAULT_DRAG_DURATION, DEFAULT_WAIT_TIMEOUT};
use crate::keys::{Key, combo_name, key_name, parse_combo, parse_key};

/// Sintaxe dos passos, uma forma por linha (ajuda da TUI e do editor).
pub const SYNTAX: &[&str] = &[
    "launch \"cmd\" [workspace=N] [class=REGEX] [focus] [always]",
    "wait_layer namespace=REGEX [timeout=30s]   (barra/painel do shell, ex. quickshell:bar)",
    "workspace 3   vai para o workspace exato (SUPER+N pode ser relativo ao monitor)",
    "wait_monitors [timeout=30s]   espera os monitores das regras de workspace e reposiciona os workspaces",
    "wait_window class=REGEX [nth=N] [timeout=30s]    sleep 500ms | 2s | 1m | 1h",
    "focus class=REGEX [nth=N] [timeout=30s]   foca a janela do app (troca de workspace se preciso)",
    "key SUPER+2      type \"texto\"      scroll -3 (negativo desce)",
    "click [left|right|middle] [hold=CTRL] X,Y   ou   click window=REGEX [nth=N] at=X,Y",
    "drag [left|right|middle] [hold=SUPER] [window=REGEX] from=X,Y to=X,Y [duration=300ms]",
    "hold SUPER | mouse_right [at=X,Y]   release SUPER | mouse_right   move X,Y [duration=300ms]",
    "call outra_rotina      # comentário",
    "em qualquer passo: optional · repeat=N · if_window=REGEX · unless_window=REGEX",
];

/// Profundidade máxima de `call` aninhado.
pub const MAX_CALL_DEPTH: usize = 8;

/// Um passo do script pós-boot.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Launch {
        command: String,
        workspace: Option<Workspace>,
        class: Option<String>,
        focus: bool,
        always: bool,
    },
    WaitWindow {
        class: String,
        /// Quantas janelas da class precisam existir.
        nth: usize,
        timeout: Duration,
    },
    /// Foca a n-ésima janela da class, esperando ela existir.
    Focus {
        class: String,
        nth: usize,
        timeout: Duration,
    },
    WaitLayer {
        namespace: String,
        timeout: Duration,
    },
    WaitMonitors(Duration),
    Workspace(Workspace),
    Sleep(Duration),
    Key(Vec<Key>),
    Type(String),
    /// Clica, com as teclas de `hold` seguradas durante o clique.
    Click {
        button: Button,
        hold: Vec<Key>,
        target: ClickTarget,
    },
    Scroll(i32),
    Call(String),
    /// Aperta e mantém até um `release` ou o fim da rotina.
    Hold {
        input: Input,
        at: Option<ClickTarget>,
    },
    Release(Input),
    Move {
        target: ClickTarget,
        duration: Duration,
    },
    /// Segura as teclas e o botão em `from`, arrasta até `to` e solta tudo.
    Drag {
        button: Button,
        hold: Vec<Key>,
        /// Janela (class, nth) das coordenadas; sem ela, são absolutas.
        window: Option<(String, usize)>,
        from: (i32, i32),
        to: (i32, i32),
        duration: Duration,
    },
}

/// Tecla ou botão do mouse que pode ficar pressionado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Key(Key),
    Mouse(Button),
}

impl Input {
    /// Lê `SUPER` ou `mouse_right`.
    fn parse(name: &str) -> Result<Self> {
        match name.strip_prefix("mouse_") {
            Some(button) => Button::parse(button)
                .map(Self::Mouse)
                .ok_or_else(|| anyhow!("botão desconhecido: {name} (mouse_left, mouse_right ou mouse_middle)")),
            None => parse_key(name).map(Self::Key),
        }
    }
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Key(key) => write!(f, "{}", key_name(*key)),
            Input::Mouse(button) => write!(f, "mouse_{}", button.name()),
        }
    }
}

/// Condições e repetição aplicáveis a qualquer passo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Guard {
    /// Falha vira aviso e a rotina segue.
    pub optional: bool,
    /// Só executa se existir janela com essa class.
    pub if_window: Option<String>,
    /// Só executa se não existir janela com essa class.
    pub unless_window: Option<String>,
    /// Quantas vezes executar (1 = normal).
    pub repeat: u32,
}

/// Passo com suas condições.
#[derive(Debug, Clone, PartialEq)]
pub struct Instruction {
    pub step: Step,
    pub guard: Guard,
}

/// Botão do mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Onde clicar: posição absoluta ou relativa a uma janela.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickTarget {
    Absolute { x: i32, y: i32 },
    /// `nth` conta as janelas da class por ordem de abertura, a partir de 1.
    Window {
        class: String,
        nth: usize,
        x: i32,
        y: i32,
    },
}

/// Linha do script com o número original, para mensagens de erro.
#[derive(Debug, Clone)]
pub struct Line {
    pub number: usize,
    pub step: Step,
    pub guard: Guard,
}

/// Interpreta os passos de uma rotina, ignorando vazios e comentários.
pub fn parse_steps<S: AsRef<str>>(lines: &[S]) -> Result<Vec<Line>> {
    lines
        .iter()
        .map(AsRef::as_ref)
        .enumerate()
        .filter(|(_, line)| !is_comment(line))
        .map(|(index, line)| {
            let number = index + 1;
            parse_line(line)
                .map(|Instruction { step, guard }| Line { number, step, guard })
                .with_context(|| format!("linha {number}: {}", line.trim()))
        })
        .collect()
}

/// Interpreta o texto de vários passos separados por linha.
#[cfg(test)]
fn parse(raw: &str) -> Result<Vec<Line>> {
    parse_steps(&raw.lines().collect::<Vec<_>>())
}

/// A linha é comentário ou vazia?
pub fn is_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// Interpreta um passo e suas condições.
pub fn parse_line(line: &str) -> Result<Instruction> {
    let tokens = tokenize(line)?;
    let (verb, args) = tokens.split_first().ok_or_else(|| anyhow!("linha vazia"))?;
    let mut options = Options::new(args);
    let guard = Guard {
        optional: options.flag("optional"),
        if_window: options.take("if_window"),
        unless_window: options.take("unless_window"),
        repeat: match options.take("repeat") {
            Some(raw) => raw.parse().ok().filter(|n| *n >= 1).context("repeat deve ser 1 ou mais")?,
            None => 1,
        },
    };
    let step = match verb.text.as_str() {
        "launch" => Step::Launch {
            command: options.positional("comando")?,
            workspace: options.take("workspace").map(Workspace),
            class: options.take("class"),
            focus: options.flag("focus"),
            always: options.flag("always"),
        },
        "wait_window" => Step::WaitWindow {
            class: options.required("class")?,
            nth: parse_nth(&mut options)?,
            timeout: parse_timeout(&mut options)?,
        },
        "focus" => Step::Focus {
            class: options.required("class")?,
            nth: parse_nth(&mut options)?,
            timeout: parse_timeout(&mut options)?,
        },
        "wait_layer" => Step::WaitLayer {
            namespace: options.required("namespace")?,
            timeout: parse_timeout(&mut options)?,
        },
        "workspace" => Step::Workspace(Workspace(options.positional("workspace")?)),
        "wait_monitors" => Step::WaitMonitors(parse_timeout(&mut options)?),
        "sleep" => Step::Sleep(parse_duration(&options.positional("duração")?)?),
        "key" => Step::Key(parse_combo(&options.positional("atalho")?)?),
        "type" => Step::Type(options.positional("texto")?),
        "click" => parse_click(&mut options)?,
        "move" => Step::Move {
            target: parse_position(&mut options)?,
            duration: parse_optional_duration(&mut options, "duration", Duration::ZERO)?,
        },
        "drag" => parse_drag(&mut options)?,
        "hold" => parse_hold(&mut options)?,
        "release" => Step::Release(Input::parse(&options.positional("tecla ou botão")?)?),
        "call" => {
            let name = options.positional("rotina")?;
            crate::config::validate_name(&name)?;
            Step::Call(name)
        }
        "scroll" => Step::Scroll(
            options
                .positional("quantidade")?
                .parse()
                .context("scroll espera um inteiro (negativo = para baixo)")?,
        ),
        other => bail!("comando desconhecido: {other}"),
    };
    options.finish()?;
    Ok(Instruction { step, guard })
}

/// Opção `timeout`, com o padrão das esperas.
fn parse_timeout(options: &mut Options) -> Result<Duration> {
    parse_optional_duration(options, "timeout", DEFAULT_WAIT_TIMEOUT)
}

/// Opção `nth` (1 = mais antiga), padrão 1.
fn parse_nth(options: &mut Options) -> Result<usize> {
    match options.take("nth") {
        Some(raw) => raw.parse().ok().filter(|n| *n >= 1).context("nth deve ser 1 ou mais"),
        None => Ok(1),
    }
}

/// `click [left|right|middle] X,Y` ou `click [botão] window=CLASS [nth=N] at=X,Y`.
fn parse_click(options: &mut Options) -> Result<Step> {
    let button = parse_button(options)?;
    let hold = parse_hold_keys(options)?;
    let target = parse_position(options)?;
    Ok(Step::Click { button, hold, target })
}

/// Opção `hold=SUPER+SHIFT`, vazia se ausente.
fn parse_hold_keys(options: &mut Options) -> Result<Vec<Key>> {
    Ok(options.take("hold").map(|raw| parse_combo(&raw)).transpose()?.unwrap_or_default())
}

/// Botão opcional antes da posição, padrão `left`.
fn parse_button(options: &mut Options) -> Result<Button> {
    match options.peek_positional().and_then(Button::parse) {
        Some(button) => {
            options.positional("botão")?;
            Ok(button)
        }
        None => Ok(Button::Left),
    }
}

/// `X,Y` absoluto ou `window=CLASS [nth=N] at=X,Y`.
fn parse_position(options: &mut Options) -> Result<ClickTarget> {
    let window = parse_window(options)?;
    let raw = match window {
        Some(_) => options.required("at")?,
        None => options.positional("posição X,Y")?,
    };
    point_target(&raw, window.as_ref())
}

/// `window=CLASS [nth=N]`, para posições relativas à janela.
fn parse_window(options: &mut Options) -> Result<Option<(String, usize)>> {
    match options.take("window") {
        Some(class) => Ok(Some((class, parse_nth(options)?))),
        None => Ok(None),
    }
}

fn point_target(raw: &str, window: Option<&(String, usize)>) -> Result<ClickTarget> {
    let (x, y) = parse_point(raw)?;
    Ok(match window {
        Some((class, nth)) => ClickTarget::Window { class: class.clone(), nth: *nth, x, y },
        None => ClickTarget::Absolute { x, y },
    })
}

/// Opção de duração `key`, ou `default` se ausente.
fn parse_optional_duration(options: &mut Options, key: &str, default: Duration) -> Result<Duration> {
    options.take(key).map_or(Ok(default), |raw| parse_duration(&raw))
}

/// `drag [botão] [hold=SUPER+SHIFT] [window=CLASS [nth=N]] from=X,Y to=X,Y [duration=..]`.
fn parse_drag(options: &mut Options) -> Result<Step> {
    let button = parse_button(options)?;
    let hold = parse_hold_keys(options)?;
    let window = parse_window(options)?;
    let from = parse_point(&options.required("from")?)?;
    let to = parse_point(&options.required("to")?)?;
    let duration = parse_optional_duration(options, "duration", DEFAULT_DRAG_DURATION)?;
    Ok(Step::Drag { button, hold, window, from, to, duration })
}

/// `hold TECLA` ou `hold mouse_BOTÃO [at=X,Y | window=CLASS [nth=N] at=X,Y]`.
fn parse_hold(options: &mut Options) -> Result<Step> {
    let input = Input::parse(&options.positional("tecla ou botão")?)?;
    let window = parse_window(options)?;
    let at = match (options.take("at"), &window) {
        (Some(raw), _) => Some(point_target(&raw, window.as_ref())?),
        (None, Some(_)) => bail!("window= precisa de at=X,Y"),
        (None, None) => None,
    };
    if at.is_some() && matches!(input, Input::Key(_)) {
        bail!("posição só vale para botão do mouse");
    }
    Ok(Step::Hold { input, at })
}

fn parse_point(raw: &str) -> Result<(i32, i32)> {
    let (x, y) = raw
        .split_once(',')
        .ok_or_else(|| anyhow!("posição deve ser X,Y: {raw}"))?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}

/// Lê `500ms`, `2s`, `1.5s`, `1m` ou `2h`.
pub fn parse_duration(raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    let (number, unit) = raw
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|i| raw.split_at(i))
        .ok_or_else(|| anyhow!("duração sem unidade (ms, s, m, h): {raw}"))?;
    let value: f64 = number.parse().with_context(|| format!("duração inválida: {raw}"))?;
    let seconds = match unit {
        "ms" => value / 1000.0,
        "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        _ => bail!("unidade inválida em {raw} (use ms, s, m ou h)"),
    };
    Ok(Duration::from_secs_f64(seconds))
}

/// Formata uma duração do jeito mais curto e exato.
pub fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

/// Token da linha; `literal` quando começou com aspas e nunca é opção.
struct Token {
    text: String,
    literal: bool,
}

/// Separa a linha em tokens, respeitando aspas e escapes.
fn tokenize(line: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut literal = false;
    let mut chars = line.trim().chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                literal |= !has_token;
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => current.push('\n'),
                            Some('t') => current.push('\t'),
                            Some(c @ ('"' | '\\')) => current.push(c),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => bail!("escape incompleto no fim da linha"),
                        },
                        Some(other) => current.push(other),
                        None => bail!("aspas sem fechamento"),
                    }
                }
            }
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(Token {
                        text: std::mem::take(&mut current),
                        literal: std::mem::take(&mut literal),
                    });
                    has_token = false;
                }
            }
            c => {
                has_token = true;
                current.push(c);
            }
        }
    }
    if has_token {
        tokens.push(Token {
            text: current,
            literal,
        });
    }
    Ok(tokens)
}

/// Coloca aspas e escapes quando o valor não pode ir cru.
pub fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && !KNOWN_FLAGS.contains(&value)
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '\\' || c == '#' || c == '=');
    if plain {
        return value.to_string();
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// Argumentos de uma linha: posicionais, `chave=valor` e flags.
struct Options {
    positional: Vec<String>,
    named: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Options {
    fn new(args: &[Token]) -> Self {
        let mut options = Self {
            positional: Vec::new(),
            named: Vec::new(),
            flags: Vec::new(),
        };
        for arg in args {
            let text = arg.text.clone();
            if arg.literal {
                options.positional.push(text);
                continue;
            }
            match text.split_once('=') {
                Some((key, value)) if is_option_name(key) => {
                    options.named.push((key.to_string(), value.to_string()));
                }
                _ if KNOWN_FLAGS.contains(&text.as_str()) => options.flags.push(text),
                _ => options.positional.push(text),
            }
        }
        options.positional.reverse();
        options
    }

    fn peek_positional(&self) -> Option<&str> {
        self.positional.last().map(String::as_str)
    }

    fn positional(&mut self, what: &str) -> Result<String> {
        self.positional.pop().ok_or_else(|| anyhow!("falta {what}"))
    }

    fn take(&mut self, key: &str) -> Option<String> {
        let index = self.named.iter().position(|(k, _)| k == key)?;
        Some(self.named.remove(index).1)
    }

    fn required(&mut self, key: &str) -> Result<String> {
        self.take(key).ok_or_else(|| anyhow!("falta {key}="))
    }

    fn flag(&mut self, name: &str) -> bool {
        let before = self.flags.len();
        self.flags.retain(|f| f != name);
        self.flags.len() != before
    }

    /// Erro se sobrou argumento que ninguém usou.
    fn finish(self) -> Result<()> {
        let leftovers: Vec<String> = self
            .positional
            .into_iter()
            .chain(self.named.into_iter().map(|(k, v)| format!("{k}={v}")))
            .chain(self.flags)
            .collect();
        if !leftovers.is_empty() {
            bail!("argumentos não reconhecidos: {}", leftovers.join(" "));
        }
        Ok(())
    }
}

/// Flags sem valor aceitas pelos comandos.
const KNOWN_FLAGS: &[&str] = &["focus", "always", "optional"];

/// Chave de opção é um identificador simples.
fn is_option_name(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

impl Button {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "middle" => Some(Self::Middle),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Launch {
                command,
                workspace,
                class,
                focus,
                always,
            } => {
                write!(f, "launch {}", quote(command))?;
                if let Some(ws) = workspace {
                    write!(f, " workspace={}", quote(&ws.0))?;
                }
                if let Some(class) = class {
                    write!(f, " class={}", quote(class))?;
                }
                if *focus {
                    f.write_str(" focus")?;
                }
                if *always {
                    f.write_str(" always")?;
                }
                Ok(())
            }
            Step::WaitWindow { class, nth, timeout } | Step::Focus { class, nth, timeout } => {
                let verb = if matches!(self, Step::Focus { .. }) { "focus" } else { "wait_window" };
                write!(f, "{verb} class={}", quote(class))?;
                if *nth > 1 {
                    write!(f, " nth={nth}")?;
                }
                if *timeout != DEFAULT_WAIT_TIMEOUT {
                    write!(f, " timeout={}", format_duration(*timeout))?;
                }
                Ok(())
            }
            Step::WaitLayer { namespace, timeout } => {
                write!(f, "wait_layer namespace={}", quote(namespace))?;
                if *timeout != DEFAULT_WAIT_TIMEOUT {
                    write!(f, " timeout={}", format_duration(*timeout))?;
                }
                Ok(())
            }
            Step::Workspace(workspace) => write!(f, "workspace {}", quote(&workspace.0)),
            Step::WaitMonitors(timeout) => {
                f.write_str("wait_monitors")?;
                if *timeout != DEFAULT_WAIT_TIMEOUT {
                    write!(f, " timeout={}", format_duration(*timeout))?;
                }
                Ok(())
            }
            Step::Sleep(duration) => write!(f, "sleep {}", format_duration(*duration)),
            Step::Key(keys) => write!(f, "key {}", combo_name(keys)),
            Step::Type(text) => write!(f, "type {}", quote(text)),
            Step::Click { button, hold, target } => {
                write!(f, "click {}", button.name())?;
                write_hold(f, hold)?;
                write_position(f, target)
            }
            Step::Scroll(amount) => write!(f, "scroll {amount}"),
            Step::Call(name) => write!(f, "call {name}"),
            Step::Hold { input, at } => {
                write!(f, "hold {input}")?;
                match at {
                    Some(ClickTarget::Absolute { x, y }) => write!(f, " at={x},{y}"),
                    Some(target) => write_position(f, target),
                    None => Ok(()),
                }
            }
            Step::Release(input) => write!(f, "release {input}"),
            Step::Move { target, duration } => {
                write!(f, "move")?;
                write_position(f, target)?;
                if !duration.is_zero() {
                    write!(f, " duration={}", format_duration(*duration))?;
                }
                Ok(())
            }
            Step::Drag { button, hold, window, from, to, duration } => {
                write!(f, "drag {}", button.name())?;
                write_hold(f, hold)?;
                if let Some((class, nth)) = window {
                    write_window(f, class, *nth)?;
                }
                write!(f, " from={},{} to={},{}", from.0, from.1, to.0, to.1)?;
                if *duration != DEFAULT_DRAG_DURATION {
                    write!(f, " duration={}", format_duration(*duration))?;
                }
                Ok(())
            }
        }
    }
}

/// ` X,Y` ou ` window=CLASS [nth=N] at=X,Y`.
fn write_position(f: &mut fmt::Formatter<'_>, target: &ClickTarget) -> fmt::Result {
    match target {
        ClickTarget::Absolute { x, y } => write!(f, " {x},{y}"),
        ClickTarget::Window { class, nth, x, y } => {
            write_window(f, class, *nth)?;
            write!(f, " at={x},{y}")
        }
    }
}

fn write_hold(f: &mut fmt::Formatter<'_>, hold: &[Key]) -> fmt::Result {
    if hold.is_empty() {
        return Ok(());
    }
    write!(f, " hold={}", combo_name(hold))
}

fn write_window(f: &mut fmt::Formatter<'_>, class: &str, nth: usize) -> fmt::Result {
    write!(f, " window={}", quote(class))?;
    if nth > 1 {
        write!(f, " nth={nth}")?;
    }
    Ok(())
}

impl fmt::Display for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(class) = &self.if_window {
            write!(f, " if_window={}", quote(class))?;
        }
        if let Some(class) = &self.unless_window {
            write!(f, " unless_window={}", quote(class))?;
        }
        if self.repeat > 1 {
            write!(f, " repeat={}", self.repeat)?;
        }
        if self.optional {
            f.write_str(" optional")?;
        }
        Ok(())
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.step, self.guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        # abre o navegador no 2
        launch "firefox --new-window" workspace=2 class=firefox
        wait_window class=firefox timeout=20s
        wait_window class=kitty nth=2
        wait_layer namespace=quickshell:bar timeout=2m
        wait_monitors timeout=1m
        workspace 3
        sleep 500ms
        key SUPER+2
        type "olá \"mundo\" # não é comentário a=b"
        click right window="org\.gnome\.Nautilus" nth=2 at=120,40
        click 10,20
        scroll -3
        launch kitty focus always
        hold SUPER
        hold mouse_right at=800,400
        move 800,700 duration=250ms
        move window=kitty at=5,5
        release mouse_right
        release SUPER
        drag right hold=SUPER from=800,400 to=800,700
        drag window=firefox nth=2 from=10,10 to=300,10 duration=1s
        click middle hold=CTRL+SHIFT 5,5
    "#;

    #[test]
    fn parses_every_step_kind() {
        let lines = parse(SAMPLE).unwrap();
        assert_eq!(lines.len(), 22);
        assert_eq!(lines[0].number, 3);
        assert!(matches!(&lines[2].step, Step::WaitWindow { nth: 2, .. }));
        assert_eq!(
            lines[8].step,
            Step::Type("olá \"mundo\" # não é comentário a=b".into())
        );
        assert_eq!(
            lines[9].step,
            Step::Click {
                button: Button::Right,
                hold: vec![],
                target: ClickTarget::Window {
                    class: r"org\.gnome\.Nautilus".into(),
                    nth: 2,
                    x: 120,
                    y: 40
                }
            }
        );
        assert!(matches!(lines[12].step, Step::Launch { focus: true, always: true, .. }));
    }

    #[test]
    fn display_roundtrips() {
        for line in parse(SAMPLE).unwrap() {
            let printed = line.step.to_string();
            let reparsed = parse(&printed).unwrap();
            assert_eq!(reparsed[0].step, line.step, "{printed}");
        }
    }

    #[test]
    fn parses_held_inputs_and_drags() {
        let lines = parse(SAMPLE).unwrap();
        assert_eq!(lines[13].step, Step::Hold { input: Input::Key(Key::LEFTMETA), at: None });
        assert_eq!(
            lines[14].step,
            Step::Hold { input: Input::Mouse(Button::Right), at: Some(ClickTarget::Absolute { x: 800, y: 400 }) }
        );
        assert_eq!(
            lines[15].step,
            Step::Move { target: ClickTarget::Absolute { x: 800, y: 700 }, duration: Duration::from_millis(250) }
        );
        assert_eq!(lines[17].step, Step::Release(Input::Mouse(Button::Right)));
        assert_eq!(
            lines[19].step,
            Step::Drag {
                button: Button::Right,
                hold: vec![Key::LEFTMETA],
                window: None,
                from: (800, 400),
                to: (800, 700),
                duration: DEFAULT_DRAG_DURATION,
            }
        );
        let Step::Drag { button: Button::Left, hold, window: Some((class, 2)), to: (300, 10), .. } = &lines[20].step else {
            panic!("{:?}", lines[20].step)
        };
        assert!(hold.is_empty() && class == "firefox");
        assert!(matches!(&lines[21].step, Step::Click { button: Button::Middle, hold, .. } if hold.len() == 2));
    }

    #[test]
    fn rejects_bad_holds_and_drags() {
        assert!(parse("hold SUPER at=1,1").is_err(), "teclas não têm posição");
        assert!(parse("hold mouse_side").is_err());
        assert!(parse("hold mouse_left window=kitty").is_err(), "window sem at");
        assert!(parse("drag from=1,1").is_err(), "drag sem to");
        assert!(parse("move").is_err());
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse("sleep 1s extra").is_err());
        assert!(parse("wait_window").is_err());
        assert!(parse("sleep 10").is_err());
        assert!(parse("dance").is_err());
    }

    #[test]
    fn parses_guards_and_call() {
        let lines = parse("call setup if_window=kitty repeat=3 optional\nkey SUPER+1 unless_window=\"fire.*\"").unwrap();
        assert_eq!(lines[0].step, Step::Call("setup".into()));
        assert_eq!(lines[0].guard.repeat, 3);
        assert!(lines[0].guard.optional);
        assert_eq!(lines[0].guard.if_window.as_deref(), Some("kitty"));
        assert_eq!(lines[1].guard.unless_window.as_deref(), Some("fire.*"));
        for line in &lines {
            let again = parse(&line.to_string()).unwrap();
            assert_eq!(again[0].guard, line.guard);
            assert_eq!(again[0].step, line.step);
        }
        assert!(parse("sleep 1s repeat=0").is_err());
        assert!(parse("call Nome").is_err());
    }
}
