use std::time::Duration;

pub const APP_NAME: &str = "pcue";
pub const CONFIG_FILE: &str = "config.toml";
pub const LOG_FILE: &str = "pcue.log";

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_STARTUP_DELAY_MS: u64 = 1500;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const SOCKET_READY_TIMEOUT: Duration = Duration::from_secs(15);

pub const BLOCK_BEGIN: &str = ">>> pcue >>>";
pub const BLOCK_END: &str = "<<< pcue <<<";

/// Nome anterior da ferramenta, para migrar config, estado e autostart.
pub const LEGACY_NAME: &str = "pwboot";

pub const DESKTOP_FIELD_CODES: &[&str] = &[
    "%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m", "@@", "@@u", "@@f",
];

pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

pub const VIRTUAL_DEVICE_NAME: &str = "pcue-virtual-input";
pub const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(400);
pub const KEY_STEP_DELAY: Duration = Duration::from_millis(12);
pub const CLICK_MOVE_SETTLE: Duration = Duration::from_millis(60);

pub const RECORD_COUNTDOWN_SECS: u64 = 3;
pub const RECORD_MIN_SLEEP: Duration = Duration::from_millis(100);
pub const RECORD_SLEEP_ROUNDING_MS: u128 = 100;
pub const IGNORED_DEVICE_PREFIXES: &[&str] = &["pcue", "ydotoold"];

pub const HISTORY_FILE: &str = "history.jsonl";
pub const DAEMON_SOCKET: &str = "daemon.sock";
pub const DAEMON_TICK: Duration = Duration::from_secs(1);
pub const EVENT_DEDUP_WINDOW: Duration = Duration::from_secs(2);
pub const TUI_RUN_DELAY_MS: u64 = 1500;

/// Eventos embutidos e quando acontecem.
pub const BUILTIN_EVENTS: &[(&str, &str)] = &[
    ("login", "ao entrar na sessão (boot ou logout/login)"),
    ("lock", "ao bloquear a tela"),
    ("unlock", "ao desbloquear a tela"),
    ("resume", "ao voltar da suspensão"),
    ("monitor_added", "ao conectar um monitor (Hyprland)"),
    ("monitor_removed", "ao desconectar um monitor (Hyprland)"),
];

/// Subdiretório de runtime com o progresso das execuções.
pub const PROGRESS_DIR: &str = "progress";
/// Subdiretório de runtime com as travas por rotina.
pub const RUNNING_DIR: &str = "running";
/// Próximos disparos publicados pelo daemon.
pub const NEXT_RUNS_FILE: &str = "next.json";

/// Evento que recebe o prefixo de espera do boot.
pub const BOOT_EVENT: &str = "login";
/// Primeira linha do prefixo de boot, usada para não repeti-lo.
pub const BOOT_MARKER: &str = "# pcue: espera o desktop carregar (boot)";
/// Quanto o prefixo de boot espera barra e monitores.
pub const BOOT_WAIT_TIMEOUT: &str = "3m";
/// Folga depois que o desktop terminou de carregar.
pub const BOOT_SETTLE: &str = "1500ms";
/// Barras conhecidas, na ordem de preferência.
pub const KNOWN_BARS: &[&str] = &["quickshell:bar", "waybar"];
