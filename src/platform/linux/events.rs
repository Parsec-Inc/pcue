use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

use anyhow::Result;

use super::hypr::{self, Hypr};
use crate::constants::DAEMON_TICK;
use crate::constants::{APP_NAME, KERNEL_UEVENTS, UEVENT_BUFFER, USB_DEVICES_DIR, USB_HUB_CLASS};
use crate::platform::{Device, Event, EventSource, Notification};
use crate::utils::log;

/// Fontes de evento disponíveis nesta sessão.
pub fn event_sources() -> Vec<EventSource> {
    let mut sources: Vec<EventSource> = vec![("logind", watch_logind), ("notificações", watch_notification_events), ("dispositivos", watch_device_events)];
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

/// Dispositivos USB como eventos do daemon.
fn watch_device_events(sender: Sender<Event>) -> Result<()> {
    watch_devices(|device| sender.send(Event::Device(device)).is_ok())
}

/// Escuta os uevents USB do kernel até `on` devolver `false`; hubs ficam de fora.
pub fn watch_devices(mut on: impl FnMut(Device) -> bool) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    addr.nl_groups = KERNEL_UEVENTS;
    let size = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
    if unsafe { libc::bind(socket.as_raw_fd(), (&raw const addr).cast(), size) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mut connected = connected_usb_devices();
    let mut buffer = vec![0u8; UEVENT_BUFFER];
    loop {
        let mut from: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut from_size = size;
        let read = unsafe {
            libc::recvfrom(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
                (&raw mut from).cast(),
                &mut from_size,
            )
        };
        if read < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }
        if from.nl_pid != 0 {
            continue;
        }
        let fields: HashMap<&str, &str> = buffer[..read as usize]
            .split(|b| *b == 0)
            .filter_map(|field| std::str::from_utf8(field).ok()?.split_once('='))
            .collect();
        if fields.get("SUBSYSTEM") != Some(&"usb") || fields.get("DEVTYPE") != Some(&"usb_device") {
            continue;
        }
        let Some(path) = fields.get("DEVPATH").map(|p| p.to_string()) else { continue };
        let device = match fields.get("ACTION") {
            Some(&"add") => match read_usb_device(&path) {
                Some(device) => {
                    connected.insert(path, device.clone());
                    device
                }
                None => continue,
            },
            Some(&"remove") => match connected.remove(&path) {
                Some(device) => Device { removed: true, ..device },
                None => match fields.get("PRODUCT").and_then(|p| usb_id(p)) {
                    Some(id) => Device { name: id.clone(), id, removed: true },
                    None => continue,
                },
            },
            _ => continue,
        };
        if !on(device) {
            return Ok(());
        }
    }
}

/// Dispositivos já conectados, por DEVPATH, para a desconexão vir com nome.
fn connected_usb_devices() -> HashMap<String, Device> {
    let Ok(entries) = std::fs::read_dir(USB_DEVICES_DIR) else { return HashMap::new() };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_name().to_string_lossy().contains(':') {
                return None;
            }
            let real = std::fs::canonicalize(entry.path()).ok()?;
            let path = real.strip_prefix("/sys").ok()?.to_str()?.to_string();
            let device = read_usb_device(&path).filter(|d| d.id != ":")?;
            Some((path, device))
        })
        .collect()
}

/// Nome e id pelo sysfs; `None` para hubs.
fn read_usb_device(path: &str) -> Option<Device> {
    let dir = std::path::Path::new("/sys").join(path.trim_start_matches('/'));
    let attr = |name: &str| std::fs::read_to_string(dir.join(name)).map(|v| v.trim().to_string()).unwrap_or_default();
    if attr("bDeviceClass") == USB_HUB_CLASS {
        return None;
    }
    let id = format!("{}:{}", attr("idVendor"), attr("idProduct"));
    let name = format!("{} {}", attr("manufacturer"), attr("product")).trim().to_string();
    Some(Device { name: if name.is_empty() { id.clone() } else { name }, id, removed: false })
}

/// `18d1/4ee7/440` (PRODUCT do uevent) vira `18d1:4ee7`.
fn usb_id(product: &str) -> Option<String> {
    let mut parts = product.split('/').map(|p| u16::from_str_radix(p, 16));
    match (parts.next(), parts.next()) {
        (Some(Ok(vendor)), Some(Ok(product))) => Some(format!("{vendor:04x}:{product:04x}")),
        _ => None,
    }
}
