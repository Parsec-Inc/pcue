use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

use anyhow::Result;

use super::hypr::{self, Hypr};
use crate::constants::DAEMON_TICK;
use crate::constants::APP_NAME;
use crate::platform::{Event, EventSource, Notification};
use crate::utils::log;

/// Fontes de evento disponíveis nesta sessão.
pub fn event_sources() -> Vec<EventSource> {
    let mut sources: Vec<EventSource> = vec![("logind", watch_logind), ("notificações", watch_notification_events)];
    if hypr::in_session() {
        sources.push(("hyprland", watch_hyprland));
        sources.push(("bloqueio", watch_lock));
    }
    sources
}

/// Bloqueio, desbloqueio e retorno da suspensão pelo logind.
fn watch_logind(sender: Sender<Event>) -> Result<()> {
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::zvariant::OwnedValue;

    let connection = Connection::system()?;
    let session = current_session(&connection);
    if session.is_none() {
        log("sessão do logind não encontrada: lock/unlock indisponíveis, resume segue ativo");
    }
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .build();

    let mut locked_hint = false;
    for message in MessageIterator::for_match_rule(rule, &connection, None)? {
        let message = message?;
        let header = message.header();
        let member = header.member().map(|m| m.as_str()).unwrap_or_default();
        let ours = header
            .path()
            .zip(session.as_ref())
            .is_some_and(|(path, session)| path.as_str() == session.as_str());
        let event = match member {
            "PrepareForSleep" => {
                let sleeping: bool = message.body().deserialize()?;
                (!sleeping).then_some("resume")
            }
            "Lock" if ours => Some("lock"),
            "Unlock" if ours => Some("unlock"),
            "PropertiesChanged" if ours => {
                let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                    message.body().deserialize()?;
                changed
                    .get("LockedHint")
                    .and_then(|v| bool::try_from(v).ok())
                    .filter(|hint| *hint != locked_hint)
                    .map(|hint| {
                        locked_hint = hint;
                        if hint { "lock" } else { "unlock" }
                    })
            }
            _ => None,
        };
        if let Some(event) = event {
            sender.send(Event::Named(event.to_string()))?;
        }
    }
    Ok(())
}

/// Caminho D-Bus da sessão: pelo PID e, se falhar, por `XDG_SESSION_ID`.
fn current_session(
    connection: &zbus::blocking::Connection,
) -> Option<zbus::zvariant::OwnedObjectPath> {
    const DEST: Option<&str> = Some("org.freedesktop.login1");
    const PATH: &str = "/org/freedesktop/login1";
    const IFACE: Option<&str> = Some("org.freedesktop.login1.Manager");
    let by_pid = connection.call_method(DEST, PATH, IFACE, "GetSessionByPID", &std::process::id());
    let reply = match by_pid {
        Ok(reply) => reply,
        Err(_) => {
            let id = std::env::var("XDG_SESSION_ID").ok()?;
            connection.call_method(DEST, PATH, IFACE, "GetSession", &id).ok()?
        }
    };
    reply.body().deserialize().ok()
}

/// Bloqueio e desbloqueio pelo estado de session lock do Hyprland.
fn watch_lock(sender: Sender<Event>) -> Result<()> {
    let hypr = Hypr::connect()?;
    let mut locked = hypr.is_locked()?;
    loop {
        std::thread::sleep(DAEMON_TICK);
        let now = hypr.is_locked()?;
        if now != locked {
            locked = now;
            sender.send(Event::Named(if now { "lock" } else { "unlock" }.to_string()))?;
        }
    }
}

/// Monitores conectados e desconectados pelo socket de eventos do Hyprland.
fn watch_hyprland(sender: Sender<Event>) -> Result<()> {
    let stream = UnixStream::connect(hypr::event_socket()?)?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        let event = match line.split_once(">>").map(|(name, _)| name) {
            Some("monitoradded" | "monitoraddedv2") => "monitor_added",
            Some("monitorremoved" | "monitorremovedv2") => "monitor_removed",
            _ => continue,
        };
        sender.send(Event::Named(event.to_string()))?;
    }
    Ok(())
}

/// Notificações dos apps como eventos do daemon.
fn watch_notification_events(sender: Sender<Event>) -> Result<()> {
    watch_notifications(|notification| sender.send(Event::Notification(notification)).is_ok())
}

/// Escuta as notificações da sessão até `on` devolver `false`; via portal, o app vem do `desktop-entry`.
pub fn watch_notifications(mut on: impl FnMut(Notification) -> bool) -> Result<()> {
    use zbus::blocking::fdo::MonitoringProxy;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::zvariant::OwnedValue;

    type Notify = (String, u32, String, String, String, Vec<String>, HashMap<String, OwnedValue>, i32);

    let connection = Connection::session()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::MethodCall)
        .interface("org.freedesktop.Notifications")?
        .member("Notify")?
        .build();
    MonitoringProxy::new(&connection)?.become_monitor(&[rule], 0)?;
    for message in MessageIterator::from(connection) {
        let Ok((name, _, _, title, body, _, hints, _)) = message?.body().deserialize::<Notify>() else {
            continue;
        };
        let app = match hints.get("desktop-entry").and_then(|v| <&str>::try_from(&**v).ok()) {
            Some(entry) if name.is_empty() => entry.to_string(),
            _ => name,
        };
        if app == APP_NAME {
            continue;
        }
        if !on(Notification { app, title, body }) {
            break;
        }
    }
    Ok(())
}
