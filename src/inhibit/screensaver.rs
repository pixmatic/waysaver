//! Minimal `org.freedesktop.ScreenSaver` D-Bus service.
//!
//! Some applications (notably Firefox) don't use Wayland idle inhibitors and
//! instead call `Inhibit`/`UnInhibit` on this well-known D-Bus name. We own the
//! name, track the active inhibitors and report whether any remain.
//!
//! Only `Inhibit` and `UnInhibit` are implemented; the object path is ignored
//! because clients disagree on it (`/ScreenSaver` vs `/org/freedesktop/ScreenSaver`).
//! Unsupported methods (including introspection) get an error reply.

use std::os::fd::RawFd;

use dbus::{
    Message, MessageType,
    channel::{BusType, Channel},
    strings::ErrorName,
};

use super::{
    InhibitSource,
    bus::{self, DBUS},
};
use crate::logging::{INFO, log};

pub const SERVICE: &str = "org.freedesktop.ScreenSaver";

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
    /// Next cookie to hand out; never 0 (some clients treat 0 as failure).
    next_cookie: u32,
}

impl ScreenSaver {
    /// Connects to the session bus and claims `org.freedesktop.ScreenSaver`.
    /// Fails if there is no session bus or another program already owns the name.
    pub fn new() -> Result<Self, String> {
        let channel = bus::connect(BusType::Session)?;

        // RequestName flag 4 = DO_NOT_QUEUE: fail instead of waiting in line.
        let reply = bus::call_daemon(&channel, "RequestName", |m| m.append2(SERVICE, 4u32))?;
        // Reply 1 = PRIMARY_OWNER; anything else means someone else owns it.
        if reply.read1::<u32>() != Ok(1) {
            return Err(format!("{SERVICE} is already owned by another program"));
        }

        // Be told when bus clients disconnect, to drop their inhibitors. This
        // also reports unrelated name changes on the bus; we filter them out.
        let rule = format!("type='signal',sender='{DBUS}',member='NameOwnerChanged'");
        bus::call_daemon(&channel, "AddMatch", |m| m.append1(rule.as_str()))?;

        Ok(Self {
            channel,
            inhibitors: Vec::new(),
            next_cookie: 1,
        })
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
                    // Wrap around skipping 0.
                    self.next_cookie = self.next_cookie.wrapping_add(1).max(1);
                    log(INFO, &format!("inhibited by {app}: {reason}"));
                    self.inhibitors.push(Inhibitor {
                        cookie,
                        // The bus always sets the sender; the default is
                        // only there to avoid an unwrap.
                        owner: msg.sender().map(|s| s.to_string()).unwrap_or_default(),
                        app: app.to_string(),
                    });
                    msg.method_return().append1(cookie)
                }
                Err(_) => error(msg, "InvalidArgs", c"expected (app, reason)"),
            },
            (Some(SERVICE), Some("UnInhibit")) => match msg.read1::<u32>() {
                // Unknown cookies are accepted silently (already released).
                Ok(cookie) => {
                    self.remove(|i| i.cookie == cookie);
                    msg.method_return()
                }
                Err(_) => error(msg, "InvalidArgs", c"expected (cookie)"),
            },
            _ => error(msg, "UnknownMethod", c"method not supported by wayvoid"),
        };
        // Callers may flag that they don't want a reply.
        if !msg.get_no_reply() {
            // Only fails if the connection is gone; dispatch() reports that.
            let _ = self.channel.send(reply);
        }
    }

    /// `NameOwnerChanged(name, old, new)` with empty `new` means `name` left
    /// the bus: forget its inhibitors (e.g. the browser crashed). Inhibitors
    /// store unique names (":1.42"), so well-known names never match.
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

impl InhibitSource for ScreenSaver {
    fn name(&self) -> &'static str {
        SERVICE
    }

    fn fd(&self) -> RawFd {
        self.channel.watch().fd
    }

    fn is_inhibited(&self) -> bool {
        !self.inhibitors.is_empty()
    }

    /// Answers method calls and tracks clients leaving the bus.
    fn dispatch(&mut self) -> Result<(), String> {
        for msg in bus::drain(&self.channel)? {
            match msg.msg_type() {
                MessageType::MethodCall => self.handle_call(&msg),
                MessageType::Signal => self.handle_signal(&msg),
                _ => {}
            }
        }
        // Send the replies queued by `handle_call` right away.
        self.channel.flush();
        Ok(())
    }
}

/// Builds a standard `org.freedesktop.DBus.Error.<name>` error reply.
fn error(msg: &Message, name: &str, text: &std::ffi::CStr) -> Message {
    let name = ErrorName::new(format!("org.freedesktop.DBus.Error.{name}"))
        .expect("valid D-Bus error name");
    msg.error(&name, text)
}
