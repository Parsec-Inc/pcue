use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_STARTUP_DELAY_MS, DEFAULT_TIMEOUT_SECS};
use crate::schedule::{Weekday, parse_clock};
use crate::script::{self, parse_duration};

/// Arquivo de configuração completo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "routine")]
    pub routines: Vec<Routine>,
}

/// Ajustes globais.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub timeout_secs: u64,
    pub startup_delay_ms: u64,
}

/// Rotina: livre quando não tem gatilhos, agendada quando tem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub steps: Vec<String>,
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
            steps: Vec::new(),
        }
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
        self.enabled
            && self
                .triggers
                .iter()
                .any(|t| matches!(t, Trigger::Event { event: e } if e == event))
    }

    /// Erros de coerência da rotina.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        for trigger in &self.triggers {
            trigger.validate()?;
        }
        script::parse_steps(&self.steps)?;
        Ok(())
    }
}

impl Trigger {
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
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
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
