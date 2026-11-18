mod boot;
mod capture;
mod config;
mod constants;
mod daemon;
mod draft;
mod history;
mod keys;
mod launcher;
mod platform;
mod player;
mod progress;
mod recorder;
mod runner;
mod schedule;
mod script;
mod tui;
mod update;
mod utils;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::runner::Selection;
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
        /// Começa na linha N do script
        #[arg(long, conflicts_with = "only")]
        from: Option<usize>,
        /// Executa só a linha N do script
        #[arg(long)]
        only: Option<usize>,
        /// Valores dos parâmetros da rotina
        #[arg(value_name = "PARÂMETRO=VALOR")]
        args: Vec<String>,
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
    /// Atualiza para a última release do GitHub
    Update {
        /// Só mostra se há versão nova
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    #[cfg(target_os = "linux")]
    platform::migrate_legacy();
    let path = config_path()?;
    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => tui::run(path),
        Command::Daemon { restarted } => daemon::run_daemon(restarted),
        Command::Run {
            routine,
            trigger,
            delay_ms,
            from,
            only,
            args,
        } => {
            let selection = Selection::from_flags(from, only);
            let args = args
                .iter()
                .map(|arg| {
                    let (name, value) = arg.split_once('=').with_context(|| format!("use PARÂMETRO=VALOR: {arg}"))?;
                    Ok((name.to_string(), value.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            std::thread::sleep(Duration::from_millis(delay_ms));
            runner::run_routine(&Config::load(&path)?, &routine, &trigger, selection, &args)
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
            let steps = record_in_terminal()?;
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
            let place = platform::install()?;
            println!("daemon registrado em {place}");
            println!("vale no próximo login; para iniciar agora: pcue daemon");
            Ok(())
        }
        Command::Uninstall => {
            match platform::uninstall()? {
                Some(place) => println!("daemon removido de {place}"),
                None => println!("pcue não estava no autostart"),
            }
            Ok(())
        }
        Command::Update { check } => update::run(check),
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

/// Gravação pela linha de comando, com o progresso impresso.
fn record_in_terminal() -> Result<Vec<String>> {
    use recorder::RecordEvent;
    println!("Troque para onde vai agir. SUPER+F9 pausa/retoma (senhas), SUPER+F10 encerra.");
    for event in recorder::start().events {
        match event {
            RecordEvent::Countdown(n) => println!("  {n}..."),
            RecordEvent::Started => println!("● gravando"),
            RecordEvent::Paused(true) => println!("‖ pausado"),
            RecordEvent::Paused(false) => println!("● gravando"),
            RecordEvent::Step(step) => println!("  {step}"),
            RecordEvent::Desktop(change) => println!("  ◆ {change}"),
            RecordEvent::Done(draft) => {
                let steps = draft.render();
                println!("■ gravação encerrada: {} passo(s)", steps.len());
                if draft.choices() > 0 {
                    println!("  (na TUI dá para escolher outras formas de cada trecho antes de salvar)");
                }
                return Ok(steps);
            }
            RecordEvent::Failed(err) => anyhow::bail!(err),
        }
    }
    anyhow::bail!("gravação interrompida")
}
