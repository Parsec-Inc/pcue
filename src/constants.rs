use std::time::Duration;

pub const APP_NAME: &str = "pcue";
pub const CONFIG_FILE: &str = "config.toml";
pub const LOG_FILE: &str = "pcue.log";

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_STARTUP_DELAY_MS: u64 = 1500;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(target_os = "linux")]
pub const SOCKET_READY_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(target_os = "linux")]
pub const BLOCK_BEGIN: &str = ">>> pcue >>>";
#[cfg(target_os = "linux")]
pub const BLOCK_END: &str = "<<< pcue <<<";

/// Nome anterior da ferramenta, para migrar config, estado e autostart.
#[cfg(target_os = "linux")]
pub const LEGACY_NAME: &str = "pwboot";

#[cfg(target_os = "linux")]
pub const DESKTOP_FIELD_CODES: &[&str] = &[
    "%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m", "@@", "@@u", "@@f",
];

pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "linux")]
pub const VIRTUAL_DEVICE_NAME: &str = "pcue-virtual-input";
#[cfg(target_os = "linux")]
pub const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(400);
pub const KEY_STEP_DELAY: Duration = Duration::from_millis(12);
pub const CLICK_MOVE_SETTLE: Duration = Duration::from_millis(60);
pub const DEFAULT_DRAG_DURATION: Duration = Duration::from_millis(300);
/// Intervalo entre os passos do cursor num movimento com duração.
pub const MOVE_FRAME: Duration = Duration::from_millis(16);
/// Distância mínima, em pixels, para a gravação tratar um clique como arrasto.
pub const DRAG_THRESHOLD: i32 = 6;
pub const RECORDED_DRAG_MIN: Duration = Duration::from_millis(100);
/// Parâmetros que o gatilho de notificação preenche: app, título e texto.
pub const NOTIFICATION_PARAMS: [&str; 3] = ["app", "titulo", "texto"];
/// Opções dos passos que não podem ser nome de parâmetro.
pub const RESERVED_PARAMS: [&str; 4] = ["optional", "if_window", "unless_window", "repeat"];
/// Gravação: intervalo entre leituras da posição do cursor.
pub const CURSOR_SAMPLE: Duration = Duration::from_millis(8);
/// Gravação: por quanto tempo guardar as posições do cursor fora de um arrasto.
pub const CURSOR_HISTORY: Duration = Duration::from_secs(5);
/// Gravação: desvio máximo, em pixels, ao enxugar o trajeto de um arrasto.
pub const DRAG_PATH_TOLERANCE: f64 = 2.0;

pub const RECORD_COUNTDOWN_SECS: u64 = 3;
pub const RECORD_MIN_SLEEP: Duration = Duration::from_millis(100);
pub const RECORD_SLEEP_ROUNDING_MS: u128 = 100;
#[cfg(target_os = "linux")]
pub const IGNORED_DEVICE_PREFIXES: &[&str] = &["pcue", "ydotoold"];

pub const HISTORY_FILE: &str = "history.jsonl";
#[cfg(target_os = "linux")]
pub const DAEMON_SOCKET: &str = "daemon.sock";
pub const DAEMON_TICK: Duration = Duration::from_secs(1);
/// Quanto o daemon novo espera o antigo liberar o canal.
pub const DAEMON_HANDOFF: Duration = Duration::from_secs(5);
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
#[cfg(target_os = "linux")]
pub const KNOWN_BARS: &[&str] = &["quickshell:bar", "waybar"];
/// No Windows, quanto a quantidade de monitores precisa ficar parada no boot.
#[cfg(windows)]
pub const MONITOR_SETTLE: Duration = Duration::from_secs(2);

/// Gravação: ações com no máximo este intervalo formam uma mesma sequência.
pub const CAUSE_GAP: Duration = Duration::from_millis(1500);
/// Gravação: o foco precisa mudar até este tempo depois do atalho para ser dele.
pub const FOCUS_ATTRIBUTION: Duration = Duration::from_millis(1500);
/// Gravação: foco na janela recém-aberta até este tempo vira `launch ... focus`.
pub const FOCUS_FOLLOW: Duration = Duration::from_millis(1500);
/// Repositório com as releases do pcue.
pub const UPDATE_REPO: &str = "Parsec-Inc/pcue";
/// Arquivo de somas SHA-256 publicado em cada release.
pub const UPDATE_CHECKSUMS: &str = "SHA256SUMS";

/// Prefixo das mudanças do desktop na lista da gravação.
pub const DESKTOP_MARK: &str = "◆";
/// Editor quando `$VISUAL`/`$EDITOR` não estão definidos.
#[cfg(target_os = "linux")]
pub const DEFAULT_EDITOR: &str = "nano";
#[cfg(windows)]
pub const DEFAULT_EDITOR: &str = "notepad";
/// Gravação: intervalo entre consultas ao desktop.
pub const DESKTOP_POLL: Duration = Duration::from_millis(150);
/// Gravação: eventos do Hyprland que pedem uma nova consulta ao desktop.
#[cfg(target_os = "linux")]
pub const DESKTOP_CHANGE_EVENTS: [&str; 6] =
    ["openwindow", "closewindow", "activewindowv2", "workspacev2", "focusedmonv2", "movewindowv2"];
