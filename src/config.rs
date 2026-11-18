use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_STARTUP_DELAY_MS, DEFAULT_TIMEOUT_SECS, RESERVED_PARAMS};
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

    /// Rotina ativa que reage ao evento.
    pub fn listens_to(&self, event: &str) -> bool {
        self.enabled && self.has_event(event)
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
            if self.is_scheduled() && param.default.is_none() {
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

    /// Confere horário, intervalo e nome do evento.
    pub fn validate(&self) -> Result<()> {
        match self {
            Trigger::Event { event } => validate_name(event).context("evento inválido"),
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
        }
    }
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

/// Lê `pessoa, mensagem=padrão` (vírgulas separam os parâmetros).
pub fn parse_params(raw: &str) -> Vec<Param> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((name, default)) => Param { name: name.trim().to_string(), default: Some(default.trim().to_string()) },
            None => Param { name: part.to_string(), default: None },
        })
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
        assert!(config.routines[0].listens_to("login"));
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
