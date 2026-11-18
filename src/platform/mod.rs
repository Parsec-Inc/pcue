//! O que muda entre Linux (Hyprland) e Windows; o resto do pcue só usa isto.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

use std::sync::mpsc::Sender;

use regex::Regex;

use crate::keys::Key;
use crate::script::Button;

/// Janela de aplicativo, no mesmo formato nos dois sistemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Identificador estável enquanto a janela existir.
    pub id: String,
    /// Hyprland: class; Windows: nome do executável sem extensão.
    pub class: String,
    /// Hyprland: class inicial; Windows: class Win32 da janela.
    pub alt_class: String,
    pub title: String,
    /// Hyprland: nome do workspace; Windows: número do monitor.
    pub workspace: String,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub pid: i64,
    /// Ordem de abertura (menor = mais antiga).
    pub opened: (u64, u64),
    /// Ordem de foco (0 = a mais recente).
    pub focus_rank: i32,
    pub on_screen: bool,
}

impl Window {
    /// O ponto global está dentro da janela?
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.at[0] && y >= self.at[1] && x < self.at[0] + self.size[0] && y < self.at[1] + self.size[1]
    }

    /// Confere a class principal ou a alternativa.
    pub fn matches(&self, re: &Regex) -> bool {
        re.is_match(&self.class) || re.is_match(&self.alt_class)
    }
}

/// Evento cru de teclado e mouse, para a gravação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawInput {
    Key { key: Key, down: bool },
    Button { button: Button, down: bool },
    Wheel(i32),
}

/// Observador de eventos do sistema, rodado numa thread pelo daemon.
pub type EventSource = (&'static str, fn(Sender<Event>) -> anyhow::Result<()>);

/// Algo que o daemon observou.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    /// Evento pelo nome: embutido ou de `pcue emit`.
    Named(String),
    #[cfg_attr(windows, allow(dead_code))]
    Notification(Notification),
}

/// Notificação de um aplicativo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Notification {
    pub app: String,
    pub title: String,
    pub body: String,
}
