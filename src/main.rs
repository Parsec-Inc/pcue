mod capture;
mod config;
mod constants;
mod daemon;
mod desktop;
mod history;
mod hypr;
mod input;
mod install;
mod keys;
mod launcher;
mod player;
mod recorder;
mod runner;
mod schedule;
mod script;
mod tui;
mod utils;

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::hypr::Hypr;
use crate::utils::{config_path, history_path, log_path};

/// Rotinas pós-login: abre apps nos workspaces, grava e reproduz ações.
#[derive(Parser)]
#[command(name = "pcue", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Abre a interface interativa (padrão)
    Tui,
    /// Processo de fundo que dispara as rotinas agendadas
    Daemon {
        /// Reinício após atualização do binário: não repete o evento login
        #[arg(long, hide = true)]
        restarted: bool,
    },
    /// Executa uma rotina agora
    Run {
        routine: String,
        /// Rótulo do gatilho para o histórico
        #[arg(long, default_value = "manual")]
        trigger: String,
        /// Espera antes de começar, em ms
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
    },
    /// Envia um evento personalizado ao daemon
    Emit { event: String },
    /// Lista as rotinas
    List,
    /// Grava teclado e mouse e acrescenta os passos à rotina
    Record { routine: String },
    /// Gera passos `launch` a partir das janelas abertas agora
    Capture {
        routine: String,
        /// Grava na rotina em vez de só mostrar
        #[arg(long)]
        save: bool,
        /// Limita a esses workspaces
        #[arg(short, long, value_delimiter = ',')]
        workspace: Vec<String>,
    },
    /// Registra o daemon no autostart da sessão
    Install,
    /// Remove o daemon do autostart
    Uninstall,
    /// Mostra os caminhos de config, log e histórico
    Paths,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    utils::migrate_legacy();
    let path = config_path()?;
    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => tui::run(path),
        Command::Daemon { restarted } => daemon::run_daemon(restarted),
        Command::Run {
            routine,
            trigger,
            delay_ms,
        } => {
            std::thread::sleep(Duration::from_millis(delay_ms));
            runner::run_routine(&Config::load(&path)?, &routine, &trigger)
        }
        Command::Emit { event } => daemon::emit(&event),
        Command::List => {
            let config = Config::load(&path)?;
            if config.routines.is_empty() {
                println!("nenhuma rotina (abra `pcue` para criar)");
            }
            for routine in &config.routines {
                let kind = if routine.is_scheduled() {
                    routine.triggers.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
                } else {
                    routine.kind_label().to_string()
                };
                let state = if routine.enabled { "" } else { " [desativada]" };
                println!("{:<20} {} passo(s)  {kind}{state}", routine.name, routine.steps.len());
            }
            Ok(())
        }
        Command::Record { routine } => {
            let steps = recorder::record(Hypr::detect().as_ref())?;
            append_steps(&path, &routine, steps)
        }
        Command::Capture {
            routine,
            save,
            workspace,
        } => {
            let steps = capture::capture(&workspace)?;
            for step in &steps {
                println!("{step}");
            }
            if save {
                append_steps(&path, &routine, steps)
            } else {
                println!("\nconfira e rode de novo com --save para gravar em '{routine}'");
                Ok(())
            }
        }
        Command::Install => {
            let file = install::install()?;
            println!("daemon registrado em {}", file.display());
            println!("vale no próximo login; para iniciar agora: pcue daemon &");
            Ok(())
        }
        Command::Uninstall => {
            match install::uninstall()? {
                Some(file) => println!("daemon removido de {}", file.display()),
                None => println!("pcue não estava no autostart"),
            }
            Ok(())
        }
        Command::Paths => {
            println!("config:    {}", path.display());
            println!("log:       {}", log_path()?.display());
            println!("histórico: {}", history_path()?.display());
            Ok(())
        }
    }
}

/// Acrescenta passos à rotina, criando-a como livre se não existir.
fn append_steps(path: &std::path::Path, name: &str, steps: Vec<String>) -> Result<()> {
    let mut config = Config::load(path)?;
    let total = steps.len();
    config.routine_or_create(name)?.steps.extend(steps);
    config.save(path)?;
    println!("{total} passo(s) acrescentados a '{name}'");
    Ok(())
}
