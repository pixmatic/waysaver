//! Minimal `org.freedesktop.ScreenSaver` D-Bus service.
//!
//! Some applications (notably Firefox) don't use Wayland idle inhibitors and
//! instead call `Inhibit`/`UnInhibit` on this well-known D-Bus name. We own the
//! name, track the active inhibitors and let `main` know whether any remain.

use std::{os::fd::RawFd, time::Duration};

use dbus::{
    Message, MessageType,
    channel::{BusType, Channel},
    strings::ErrorName,
};

use crate::{INFO, WARNING, log};

const SERVICE: &str = "org.freedesktop.ScreenSaver";
const BUS: &str = "org.freedesktop.DBus";
/// Timeout for the few blocking calls made at startup.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// An active inhibition request.
struct Inhibitor {
    cookie: u32,
    /// Unique bus name of the caller, used to drop it if the caller exits.
    owner: String,
    app: String,
}

pub struct ScreenSaver {
    channel: Channel,
    inhibitors: Vec<Inhibitor>,
    next_cookie: u32,
}

impl ScreenSaver {
    /// Connects to the session bus and claims `org.freedesktop.ScreenSaver`.
    /// Fails if there is no session bus or another program already owns the name.
    pub fn new() -> Result<Self, String> {
        let mut channel =
            Channel::get_private(BusType::Session).map_err(|e| format!("session bus: {e}"))?;
        // Lets us retrieve the socket fd and poll it alongside Wayland.
        channel.set_watch_enabled(true);

        // RequestName flag 4 = DO_NOT_QUEUE: fail instead of waiting in line.
        let reply = call(&channel, "RequestName", |m| m.append2(SERVICE, 4u32))?;
        // Reply 1 = PRIMARY_OWNER; anything else means someone else owns it.
        if reply.read1::<u32>() != Ok(1) {
            return Err(format!("{SERVICE} is already owned by another program"));
        }

        // Be told when bus clients disconnect, to drop their inhibitors.
        let rule = format!("type='signal',sender='{BUS}',member='NameOwnerChanged'");
        call(&channel, "AddMatch", |m| m.append1(rule.as_str()))?;

        Ok(Self {
            channel,
            inhibitors: Vec::new(),
            next_cookie: 1,
        })
    }

    /// The bus socket, to be polled for readability.
    pub fn fd(&self) -> RawFd {
        self.channel.watch().fd
    }

    pub fn is_inhibited(&self) -> bool {
        !self.inhibitors.is_empty()
    }

    /// Reads and handles every pending message without blocking.
    /// Returns an error if the bus connection is lost.
    pub fn dispatch(&mut self) -> Result<(), String> {
        self.channel
            .read_write(Some(Duration::ZERO))
            .map_err(|()| "session bus connection lost".to_string())?;
        while let Some(msg) = self.channel.pop_message() {
            match msg.msg_type() {
                MessageType::MethodCall => self.handle_call(&msg),
                MessageType::Signal => self.handle_signal(&msg),
                _ => {}
            }
        }
        self.channel.flush();
        Ok(())
    }

    /// Handles `Inhibit(app, reason) -> cookie` and `UnInhibit(cookie)`.
    /// Any other method gets an `UnknownMethod` error.
    fn handle_call(&mut self, msg: &Message) {
        let iface = msg.interface();
        let member = msg.member();
        let reply = match (iface.as_deref(), member.as_deref()) {
            (Some(SERVICE), Some("Inhibit")) => match msg.read2::<&str, &str>() {
                Ok((app, reason)) => {
                    let cookie = self.next_cookie;
                    self.next_cookie = self.next_cookie.wrapping_add(1).max(1);
                    log(INFO, &format!("inhibited by {app}: {reason}"));
                    self.inhibitors.push(Inhibitor {
                        cookie,
                        owner: msg.sender().map(|s| s.to_string()).unwrap_or_default(),
                        app: app.to_string(),
                    });
                    msg.method_return().append1(cookie)
                }
                Err(_) => error(msg, "InvalidArgs", c"expected (app, reason)"),
            },
            (Some(SERVICE), Some("UnInhibit")) => match msg.read1::<u32>() {
                Ok(cookie) => {
                    self.remove(|i| i.cookie == cookie);
                    msg.method_return()
                }
                Err(_) => error(msg, "InvalidArgs", c"expected (cookie)"),
            },
            _ => error(msg, "UnknownMethod", c"method not supported by wayvoid"),
        };
        if !msg.get_no_reply() {
            // Only fails if the connection is gone; dispatch() reports that.
            let _ = self.channel.send(reply);
        }
    }

    /// `NameOwnerChanged(name, old, new)` with empty `new` means `name` left
    /// the bus: forget its inhibitors (e.g. the browser crashed).
    fn handle_signal(&mut self, msg: &Message) {
        if msg.member().as_deref() != Some("NameOwnerChanged") {
            return;
        }
        if let Ok((name, _, "")) = msg.read3::<&str, &str, &str>() {
            self.remove(|i| i.owner == name);
        }
    }

    /// Drops matching inhibitors, logging each one.
    fn remove(&mut self, matches: impl Fn(&Inhibitor) -> bool) {
        self.inhibitors.retain(|i| {
            let keep = !matches(i);
            if !keep {
                log(INFO, &format!("released by {}", i.app));
            }
            keep
        });
    }
}

/// Blocking call to the bus daemon itself (`org.freedesktop.DBus`).
fn call(
    channel: &Channel,
    method: &str,
    args: impl FnOnce(Message) -> Message,
) -> Result<Message, String> {
    let msg = Message::new_method_call(BUS, "/org/freedesktop/DBus", BUS, method)?;
    channel
        .send_with_reply_and_block(args(msg), CALL_TIMEOUT)
        .map_err(|e| format!("{method}: {e}"))
}

/// Builds a standard `org.freedesktop.DBus.Error.<name>` error reply.
fn error(msg: &Message, name: &str, text: &std::ffi::CStr) -> Message {
    let name = ErrorName::new(format!("org.freedesktop.DBus.Error.{name}"))
        .expect("valid D-Bus error name");
    msg.error(&name, text)
}

/// Logged when the service can't start; wayvoid then relies on Wayland only.
pub fn warn_unavailable(reason: &str) {
    log(
        WARNING,
        &format!("D-Bus inhibitors disabled ({reason}); only Wayland inhibitors apply"),
    );
}
