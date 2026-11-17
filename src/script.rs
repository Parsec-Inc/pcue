use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use evdev::Key;

use crate::config::Workspace;
use crate::constants::DEFAULT_WAIT_TIMEOUT;
use crate::keys::{combo_name, parse_combo};

/// Sintaxe dos passos, uma forma por linha (ajuda da TUI e do editor).
pub const SYNTAX: &[&str] = &[
    "launch \"cmd\" [workspace=N] [class=REGEX] [focus] [always]",
    "wait_layer namespace=REGEX [timeout=30s]   (barra/painel do shell, ex. quickshell:bar)",
    "workspace 3   vai para o workspace exato (SUPER+N pode ser relativo ao monitor)",
    "wait_monitors [timeout=30s]   espera os monitores das regras de workspace e reposiciona os workspaces",
    "wait_window class=REGEX [nth=N] [timeout=30s]    sleep 500ms | 2s | 1m | 1h",
    "key SUPER+2      type \"texto\"      scroll -3 (negativo desce)",
    "click [left|right|middle] X,Y   ou   click window=REGEX [nth=N] at=X,Y",
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
    WaitLayer {
        namespace: String,
        timeout: Duration,
    },
    WaitMonitors(Duration),
    Workspace(Workspace),
    Sleep(Duration),
    Key(Vec<Key>),
    Type(String),
    Click {
        button: Button,
        target: ClickTarget,
    },
    Scroll(i32),
    Call(String),
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
    Ok(options
        .take("timeout")
        .map(|v| parse_duration(&v))
        .transpose()?
        .unwrap_or(DEFAULT_WAIT_TIMEOUT))
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
    let mut button = Button::Left;
    if let Some(name) = options.peek_positional()
        && let Some(parsed) = Button::parse(name) {
            button = parsed;
            options.positional("botão")?;
        }
    let target = match options.take("window") {
        Some(class) => {
            let (x, y) = parse_point(&options.required("at")?)?;
            let nth = parse_nth(options)?;
            ClickTarget::Window { class, nth, x, y }
        }
        None => {
            let (x, y) = parse_point(&options.positional("posição X,Y")?)?;
            ClickTarget::Absolute { x, y }
        }
    };
    Ok(Step::Click { button, target })
}

/// Lê `X,Y`.
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
fn format_duration(duration: Duration) -> String {
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

    /// Código evdev do botão.
    pub fn key(self) -> Key {
        match self {
            Self::Left => Key::BTN_LEFT,
            Self::Right => Key::BTN_RIGHT,
            Self::Middle => Key::BTN_MIDDLE,
        }
    }

    /// Botão correspondente ao código evdev.
    pub fn from_key(key: Key) -> Option<Self> {
        match key {
            Key::BTN_LEFT => Some(Self::Left),
            Key::BTN_RIGHT => Some(Self::Right),
            Key::BTN_MIDDLE => Some(Self::Middle),
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
            Step::WaitWindow { class, nth, timeout } => {
                write!(f, "wait_window class={}", quote(class))?;
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
            Step::Click { button, target } => {
                write!(f, "click {}", button.name())?;
                match target {
                    ClickTarget::Absolute { x, y } => write!(f, " {x},{y}"),
                    ClickTarget::Window { class, nth, x, y } => {
                        write!(f, " window={}", quote(class))?;
                        if *nth > 1 {
                            write!(f, " nth={nth}")?;
                        }
                        write!(f, " at={x},{y}")
                    }
                }
            }
            Step::Scroll(amount) => write!(f, "scroll {amount}"),
            Step::Call(name) => write!(f, "call {name}"),
        }
    }
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
    "#;

    #[test]
    fn parses_every_step_kind() {
        let lines = parse(SAMPLE).unwrap();
        assert_eq!(lines.len(), 13);
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
