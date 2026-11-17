use std::time::Duration;

pub const APP_NAME: &str = "pwboot";
pub const CONFIG_FILE: &str = "config.toml";
pub const LOG_FILE: &str = "pwboot.log";

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_STARTUP_DELAY_MS: u64 = 1500;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const SOCKET_READY_TIMEOUT: Duration = Duration::from_secs(15);

pub const BLOCK_BEGIN: &str = ">>> pwboot >>>";
pub const BLOCK_END: &str = "<<< pwboot <<<";

pub const DESKTOP_FIELD_CODES: &[&str] = &[
    "%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m", "@@", "@@u", "@@f",
];

pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

pub const VIRTUAL_DEVICE_NAME: &str = "pwboot-virtual-input";
pub const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(400);
pub const KEY_STEP_DELAY: Duration = Duration::from_millis(12);
pub const CLICK_MOVE_SETTLE: Duration = Duration::from_millis(60);

pub const RECORD_COUNTDOWN_SECS: u64 = 3;
pub const RECORD_MIN_SLEEP: Duration = Duration::from_millis(100);
pub const RECORD_SLEEP_ROUNDING_MS: u128 = 100;
pub const IGNORED_DEVICE_PREFIXES: &[&str] = &["pwboot", "ydotoold"];

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
