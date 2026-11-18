use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use regex::{Regex, RegexBuilder};

use crate::constants::{DEFAULT_STARTUP_DELAY_MS, DEFAULT_TIMEOUT_SECS, DEVICE_PARAMS, NOTIFICATION_PARAMS, RESERVED_PARAMS};
use crate::platform::Event;
use crate::schedule::{Weekday, parse_clock};
use crate::script::{self, Arg, Line, is_comment, parse_duration};

/// Arquivo de configuração completo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "routine")]
    pub routines: Vec<Routine>,
}

/// Rotinas avulsas para exportar e importar.
#[derive(Debug, Serialize, Deserialize)]
struct Bundle {
    #[serde(default, rename = "routine")]
    routines: Vec<Routine>,
}

/// Ajustes globais.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub timeout_secs: u64,
    pub startup_delay_ms: u64,
}

/// Rotina: livre quando não tem gatilhos, agendada quando tem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default)]
    pub steps: Vec<String>,
}

/// Parâmetro de rotina, usado nos passos como `{nome}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Quando uma rotina agendada dispara.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Event {
        event: String,
    },
    Daily {
        at: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        days: Vec<Weekday>,
    },
    Interval {
        every: String,
    },
    /// Notificação de app; cada filtro é regex sem diferenciar maiúsculas e vazio aceita tudo.
    Notification {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        app: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        body: String,
    },
    /// Dispositivo USB conectado (ou desconectado, com `removed`); `name` é regex do nome ou do id.
    Device {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        removed: bool,
    },
}

/// Workspace alvo: número, `name:x` ou `special:x`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Workspace(pub String);

impl Workspace {
    /// Nome do workspace como o Hyprland reporta nas janelas.
    #[cfg(target_os = "linux")]
    pub fn reported_name(&self) -> &str {
        self.0.strip_prefix("name:").unwrap_or(&self.0)
    }
}

impl fmt::Display for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            startup_delay_ms: DEFAULT_STARTUP_DELAY_MS,
        }
    }
}

impl Routine {
    /// Rotina livre e vazia.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            enabled: true,
            triggers: Vec::new(),
            params: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// Valores de todos os parâmetros: os passados e, no resto, o padrão.
    pub fn bind(&self, args: &[Arg]) -> Result<Vec<Arg>> {
        if let Some((name, _)) = args.iter().find(|(name, _)| !self.params.iter().any(|p| p.name == *name)) {
            bail!("'{}' não tem o parâmetro {name}", self.name);
        }
        self.params
            .iter()
            .map(|param| {
                let given = args.iter().rev().find(|(name, _)| *name == param.name).map(|(_, v)| v);
                let value = given.or(param.default.as_ref()).with_context(|| format!("falta o parâmetro {0} ({0}=VALOR)", param.name))?;
                Ok((param.name.clone(), value.clone()))
            })
            .collect()
    }

    /// Passos prontos para rodar com os valores passados.
    pub fn lines(&self, args: &[Arg]) -> Result<Vec<Line>> {
        script::parse_steps_with(&self.steps, &self.bind(args)?)
    }

    /// Tem gatilhos, logo roda pelo daemon.
    pub fn is_scheduled(&self) -> bool {
        !self.triggers.is_empty()
    }

    /// Rótulo do tipo para a interface.
    pub fn kind_label(&self) -> &'static str {
        if self.is_scheduled() { "agendada" } else { "livre" }
    }

    /// Declara o parâmetro `name`?
    pub fn has_param(&self, name: &str) -> bool {
        self.params.iter().any(|p| p.name == name)
    }

    /// Tem gatilho para o evento, ativa ou não.
    pub fn has_event(&self, event: &str) -> bool {
        self.triggers.iter().any(|t| t.is_event(event))
    }

    /// Erros de coerência da rotina.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        for trigger in &self.triggers {
            trigger.validate()?;
        }
        let mut names = HashSet::new();
        for param in &self.params {
            validate_name(&param.name).context("parâmetro inválido")?;
            if RESERVED_PARAMS.contains(&param.name.as_str()) || !names.insert(param.name.as_str()) {
                bail!("parâmetro '{}' repetido ou reservado", param.name);
            }
            if param.default.is_none() && self.triggers.iter().any(|t| !t.provides().contains(&param.name.as_str())) {
                bail!("rotina agendada precisa de padrão para o parâmetro {}", param.name);
            }
        }
        for (index, line) in self.steps.iter().enumerate().filter(|(_, l)| !is_comment(l)) {
            self.check_line(line).with_context(|| format!("linha {}: {}", index + 1, line.trim()))?;
        }
        Ok(())
    }

    /// Confere um passo com os padrões; com parâmetro sem padrão, só ao rodar.
    pub fn check_line(&self, line: &str) -> Result<()> {
        let defaults: Vec<Arg> = self
            .params
            .iter()
            .filter_map(|p| p.default.clone().map(|value| (p.name.clone(), value)))
            .collect();
        match script::parse_line_with(line, &defaults) {
            Err(_) if self.params.iter().any(|p| p.default.is_none() && line.contains(&format!("{{{}}}", p.name))) => {
                Ok(())
            }
            other => other.map(|_| ()),
        }
    }
}

impl Trigger {
    /// Gatilho do evento `name`.
    pub fn is_event(&self, name: &str) -> bool {
        matches!(self, Trigger::Event { event } if event == name)
    }

    /// O gatilho dispara com este evento?
    pub fn catches(&self, event: &Event) -> bool {
        match (self, event) {
            (Trigger::Event { event }, Event::Named(name)) => event == name,
            (Trigger::Notification { app, title, body }, Event::Notification(notification)) => {
                [(app, &notification.app), (title, &notification.title), (body, &notification.body)]
                    .into_iter()
                    .all(|(filter, value)| accepts(filter, value))
            }
            (Trigger::Device { name, removed }, Event::Device(device)) => {
                *removed == device.removed && (accepts(name, &device.name) || accepts(name, &device.id))
            }
            _ => false,
        }
    }

    /// Parâmetros que o gatilho preenche ao disparar.
    pub fn provides(&self) -> &'static [&'static str] {
        match self {
            Trigger::Notification { .. } => &NOTIFICATION_PARAMS,
            Trigger::Device { .. } => &DEVICE_PARAMS,
            _ => &[],
        }
    }

    /// Gatilho de notificação a partir de `app=x, titulo=y, texto=z`.
    pub fn parse_notification(raw: &str) -> Result<Self> {
        let (mut app, mut title, mut body) = (String::new(), String::new(), String::new());
        for (key, value) in key_values(raw) {
            let value = value.with_context(|| format!("use chave=valor em '{key}'"))?;
            let slot = match key {
                "app" => &mut app,
                "titulo" | "título" => &mut title,
                "texto" => &mut body,
                other => bail!("filtro desconhecido '{other}' (app, titulo, texto)"),
            };
            *slot = unquote(value).to_string();
        }
        let trigger = Trigger::Notification { app, title, body };
        trigger.validate()?;
        Ok(trigger)
    }

    /// Filtros no formato que `parse_notification` lê.
    pub fn notification_input(&self) -> String {
        let Trigger::Notification { app, title, body } = self else {
            return String::new();
        };
        [("app", app), ("titulo", title), ("texto", body)]
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Confere horário, intervalo, nome do evento e filtros.
    pub fn validate(&self) -> Result<()> {
        match self {
            Trigger::Event { event } => validate_name(event).context("evento inválido"),
            Trigger::Notification { app, title, body } => {
                [app, title, body].into_iter().try_for_each(|filter| check_filter(filter))
            }
            Trigger::Device { name, .. } => check_filter(name),
            Trigger::Daily { at, .. } => parse_clock(at).map(|_| ()),
            Trigger::Interval { every } => {
                if parse_duration(every)?.as_secs() < 60 {
                    bail!("intervalo mínimo é 1m");
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Event { event } => write!(f, "evento {event}"),
            Trigger::Daily { at, days } if days.is_empty() => write!(f, "todo dia às {at}"),
            Trigger::Daily { at, days } => {
                let names: Vec<&str> = days.iter().map(|d| d.label()).collect();
                write!(f, "às {at} ({})", names.join(", "))
            }
            Trigger::Interval { every } => write!(f, "a cada {every}"),
            Trigger::Notification { .. } => match self.notification_input() {
                filters if filters.is_empty() => write!(f, "qualquer notificação"),
                filters => write!(f, "notificação {filters}"),
            },
            Trigger::Device { name, removed } => {
                let when = if *removed { "desconectado" } else { "conectado" };
                match name.as_str() {
                    "" => write!(f, "qualquer dispositivo {when}"),
                    name => write!(f, "dispositivo {name} {when}"),
                }
            }
        }
    }
}

/// Valores do evento para os parâmetros que o gatilho preenche.
pub fn event_args(event: &Event) -> Vec<Arg> {
    let (names, values): (&[&str], Vec<&String>) = match event {
        Event::Named(_) => (&[], Vec::new()),
        Event::Notification(n) => (&NOTIFICATION_PARAMS, vec![&n.app, &n.title, &n.body]),
        Event::Device(d) => (&DEVICE_PARAMS, vec![&d.name, &d.id]),
    };
    names.iter().zip(values).map(|(name, value)| (name.to_string(), value.clone())).collect()
}

/// Como o evento aparece no log e no histórico.
pub fn event_label(event: &Event) -> String {
    match event {
        Event::Named(name) => format!("evento {name}"),
        Event::Notification(n) => format!("notificação de {}", n.app),
        Event::Device(d) => format!("{} {}", d.name, if d.removed { "desconectado" } else { "conectado" }),
    }
}

/// Tira um par de aspas em volta do valor, se houver.
fn unquote(value: &str) -> &str {
    ['"', '\'']
        .into_iter()
        .find_map(|q| value.strip_prefix(q).and_then(|v| v.strip_suffix(q)))
        .unwrap_or(value)
}

/// Filtro vazio aceita tudo; o resto é regex sem diferenciar maiúsculas.
fn accepts(filter: &str, value: &str) -> bool {
    filter.is_empty() || filter_regex(filter).is_ok_and(|re| re.is_match(value))
}

fn check_filter(filter: &str) -> Result<()> {
    filter_regex(filter).map(|_| ()).with_context(|| format!("filtro inválido: {filter}"))
}

/// Regex sem diferenciar maiúsculas, como nos filtros dos gatilhos.
pub fn filter_regex(filter: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(filter).case_insensitive(true).build()
}

impl Config {
    /// Lê o arquivo; inexistente vira configuração vazia.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("falha ao ler {}", path.display()))?;
        let config: Self =
            toml::from_str(&raw).with_context(|| format!("TOML inválido em {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Valida e grava de forma atômica, legível só pelo dono.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let dir = path.parent().context("caminho de config sem diretório")?;
        std::fs::create_dir_all(dir)?;
        let tmp = path.with_extension("toml.tmp");
        let mut file = crate::platform::private_file(&tmp)?;
        file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path).with_context(|| format!("falha ao gravar {}", path.display()))
    }

    /// Procura uma rotina pelo nome.
    pub fn find(&self, name: &str) -> Option<&Routine> {
        self.routines.iter().find(|r| r.name == name)
    }

    /// Rotina pelo nome, criada como livre se não existir.
    pub fn routine_or_create(&mut self, name: &str) -> Result<&mut Routine> {
        validate_name(name)?;
        let index = match self.routines.iter().position(|r| r.name == name) {
            Some(index) => index,
            None => {
                self.routines.push(Routine::new(name));
                self.routines.len() - 1
            }
        };
        Ok(&mut self.routines[index])
    }

    /// `base`, ou `base_2`, `base_3`... se já existir.
    pub fn unique_name(&self, base: &str) -> String {
        (1..)
            .map(|n| if n == 1 { base.to_string() } else { format!("{base}_{n}") })
            .find(|name| self.find(name).is_none())
            .expect("sempre há um nome livre")
    }

    /// Copia a rotina com nome novo; a cópia nasce desativada.
    pub fn duplicate(&mut self, index: usize) -> Result<String> {
        let mut copy = self.routines.get(index).context("rotina inexistente")?.clone();
        copy.name = self.unique_name(&format!("{}_copia", copy.name));
        copy.enabled = false;
        let name = copy.name.clone();
        self.routines.insert(index + 1, copy);
        Ok(name)
    }

    /// Grava a rotina num arquivo TOML avulso.
    pub fn export(&self, name: &str, path: &Path) -> Result<()> {
        let routine = self.find(name).context("rotina inexistente")?.clone();
        let bundle = Bundle { routines: vec![routine] };
        std::fs::write(path, toml::to_string_pretty(&bundle)?)
            .with_context(|| format!("falha ao gravar {}", path.display()))
    }

    /// Acrescenta as rotinas do arquivo, renomeando nomes repetidos.
    pub fn import(&mut self, path: &Path) -> Result<Vec<String>> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("falha ao ler {}", path.display()))?;
        let bundle: Bundle = toml::from_str(&raw).context("arquivo não é uma rotina do pcue")?;
        if bundle.routines.is_empty() {
            bail!("nenhuma rotina no arquivo");
        }
        let mut names = Vec::new();
        for mut routine in bundle.routines {
            routine.name = self.unique_name(&routine.name);
            names.push(routine.name.clone());
            self.routines.push(routine);
        }
        self.validate()?;
        Ok(names)
    }

    /// Nomes únicos e rotinas coerentes.
    pub fn validate(&self) -> Result<()> {
        let mut names = HashSet::new();
        for routine in &self.routines {
            if !names.insert(routine.name.as_str()) {
                bail!("rotina '{}' aparece mais de uma vez", routine.name);
            }
            routine
                .validate()
                .with_context(|| format!("rotina '{}'", routine.name))?;
        }
        Ok(())
    }
}

/// Itens `chave` ou `chave=valor` separados por vírgula.
fn key_values(raw: &str) -> impl Iterator<Item = (&str, Option<&str>)> {
    raw.split(',').map(str::trim).filter(|part| !part.is_empty()).map(|part| match part.split_once('=') {
        Some((key, value)) => (key.trim(), Some(value.trim())),
        None => (part, None),
    })
}

/// Lê `pessoa, mensagem=padrão` (vírgulas separam os parâmetros).
pub fn parse_params(raw: &str) -> Vec<Param> {
    key_values(raw)
        .map(|(name, default)| Param { name: name.to_string(), default: default.map(str::to_string) })
        .collect()
}

/// Parâmetros no formato que `parse_params` lê.
pub fn params_label(params: &[Param]) -> String {
    let label = |p: &Param| match &p.default {
        Some(default) => format!("{}={default}", p.name),
        None => p.name.clone(),
    };
    params.iter().map(label).collect::<Vec<_>>().join(", ")
}

/// Nomes de rotina e evento: minúsculas, dígitos, `-` e `_`.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        bail!("nome '{name}' inválido: use minúsculas, dígitos, '-' ou '_'");
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Device, Notification};

    #[test]
    fn params_bind_with_defaults_and_validate() {
        let mut routine = Routine::new("teams-msg");
        routine.params = parse_params("pessoa, mensagem=safeeerrr");
        routine.steps = vec!["type {pessoa}".into(), "sleep {pausa}".into(), "type {mensagem}".into()];
        assert!(routine.validate().is_err(), "{{pausa}} não é parâmetro, então sleep {{pausa}} é inválido");
        routine.steps[1] = "sleep 1s".into();
        routine.validate().unwrap();
        assert_eq!(params_label(&routine.params), "pessoa, mensagem=safeeerrr");

        let args = routine.bind(&[("pessoa".into(), "erick".into())]).unwrap();
        assert_eq!(args, vec![("pessoa".into(), "erick".into()), ("mensagem".into(), "safeeerrr".into())]);
        assert!(routine.bind(&[]).is_err(), "pessoa não tem padrão");
        assert!(routine.bind(&[("x".into(), "1".into())]).is_err(), "parâmetro desconhecido");

        routine.triggers.push(Trigger::Event { event: "ping".into() });
        assert!(routine.validate().is_err(), "agendada precisa de padrão");
        routine.params = parse_params("repeat=1");
        assert!(routine.validate().is_err(), "nome reservado");
    }

    #[test]
    fn notification_trigger_filters_and_fills_params() {
        let trigger = Trigger::parse_notification("app=teams, título=\"erick\"").unwrap();
        assert_eq!(trigger.notification_input(), "app=teams, titulo=erick");
        assert_eq!(trigger.to_string(), "notificação app=teams, titulo=erick");
        let notification = Notification {
            app: "com.github.IsmaelMartinez.teams_for_linux".into(),
            title: "Erick Silva".into(),
            body: "safe?".into(),
        };
        let teams = Event::Notification(notification.clone());
        assert!(trigger.catches(&teams));
        assert!(!trigger.catches(&Event::Notification(Notification { title: "Ana".into(), ..notification })));
        assert!(Trigger::parse_notification("").unwrap().catches(&teams), "sem filtro aceita tudo");
        assert!(Trigger::parse_notification("app=(").is_err(), "regex inválida");
        assert!(Trigger::parse_notification("de=erick").is_err(), "filtro desconhecido");

        let mut routine = Routine::new("responde");
        routine.params = parse_params("titulo, texto");
        routine.steps = vec!["type {titulo}: {texto}".into()];
        routine.triggers.push(trigger);
        routine.validate().unwrap();
        let args = event_args(&teams);
        assert_eq!(routine.bind(&args[1..]).unwrap()[0].1, "Erick Silva");
        routine.triggers.push(Trigger::Event { event: "ping".into() });
        assert!(routine.validate().is_err(), "evento não preenche titulo");
    }

    #[test]
    fn device_trigger_matches_name_or_id_and_direction() {
        let phone = Device { name: "Google Pixel 8".into(), id: "18d1:4ee7".into(), removed: false };
        let plugged = Event::Device(phone.clone());
        let unplugged = Event::Device(Device { removed: true, ..phone });
        let by_name = Trigger::Device { name: "pixel".into(), removed: false };
        assert!(by_name.catches(&plugged));
        assert!(!by_name.catches(&unplugged), "só ao conectar");
        assert!(Trigger::Device { name: "18d1:".into(), removed: true }.catches(&unplugged));
        assert!(Trigger::Device { name: String::new(), removed: false }.catches(&plugged));
        assert!(!Trigger::Device { name: "iphone".into(), removed: false }.catches(&plugged));
        assert_eq!(by_name.to_string(), "dispositivo pixel conectado");
        assert_eq!(event_args(&plugged)[1], ("id".into(), "18d1:4ee7".into()));
        assert_eq!(event_label(&unplugged), "Google Pixel 8 desconectado");

        let toml = toml::to_string(&by_name).unwrap();
        assert!(!toml.contains("removed"), "conectar é o padrão: {toml}");
    }

    const SAMPLE: &str = r#"
        [[routine]]
        name = "boot"
        triggers = [
            { type = "event", event = "login" },
            { type = "daily", at = "09:00", days = ["mon", "fri"] },
            { type = "interval", every = "30m" },
        ]
        steps = ["launch firefox workspace=2 class=firefox", "key SUPER+2"]

        [[routine]]
        name = "reunião"
        steps = []
    "#;

    #[test]
    fn parses_routines_and_rejects_bad_names() {
        let config: Config = toml::from_str(SAMPLE).unwrap();
        assert!(config.routines[0].validate().is_ok());
        assert!(config.routines[0].has_event("login"));
        assert_eq!(config.routines[0].triggers[1].to_string(), "às 09:00 (seg, sex)");
        assert!(config.validate().is_err());
    }

    #[test]
    fn kind_follows_triggers() {
        let mut routine = Routine::new("x");
        assert_eq!(routine.kind_label(), "livre");
        routine.triggers.push(Trigger::Event { event: "login".into() });
        assert_eq!(routine.kind_label(), "agendada");
        assert!(routine.validate().is_ok());
        routine.triggers = vec![Trigger::Interval { every: "10s".into() }];
        assert!(routine.validate().is_err());
    }
}
