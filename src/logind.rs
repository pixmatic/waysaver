//! Watches systemd-logind for `idle` inhibitor locks (`systemd-inhibit`).
//!
//! logind lists the currently blocked operations in the `BlockInhibited`
//! property (e.g. "idle:sleep") and announces changes with
//! `PropertiesChanged`, so this is event-driven: no polling needed.

use std::os::fd::RawFd;

use dbus::{
    Message,
    arg::{PropMap, RefArg, Variant},
    channel::{BusType, Channel},
};

use crate::{
    INFO,
    bus::{self, InhibitSource},
    log,
};

const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

pub struct Logind {
    channel: Channel,
    idle_blocked: bool,
}

impl Logind {
    /// Connects to the system bus, subscribes to Manager property changes
    /// and reads the initial `BlockInhibited` value.
    pub fn new() -> Result<Self, String> {
        let channel = bus::connect(BusType::System)?;

        let rule = format!(
            "type='signal',sender='{LOGIND}',path='{PATH}',\
             interface='{PROPERTIES}',member='PropertiesChanged',arg0='{MANAGER}'"
        );
        bus::call_daemon(&channel, "AddMatch", |m| m.append1(rule.as_str()))?;

        let reply = bus::call(&channel, (LOGIND, PATH, PROPERTIES), "Get", |m| {
            m.append2(MANAGER, "BlockInhibited")
        })?;
        let Variant(blocked) = reply
            .read1::<Variant<String>>()
            .map_err(|e| format!("BlockInhibited: {e}"))?;

        let mut logind = Self {
            channel,
            idle_blocked: false,
        };
        logind.update(&blocked);
        Ok(logind)
    }

    /// Updates the state from a `BlockInhibited` value ("a:b:c"), logging changes.
    fn update(&mut self, blocked: &str) {
        let idle = blocked.split(':').any(|op| op == "idle");
        if idle != self.idle_blocked {
            self.idle_blocked = idle;
            log(
                INFO,
                if idle {
                    "inhibited by a logind idle lock (systemd-inhibit)"
                } else {
                    "logind idle lock released"
                },
            );
        }
    }
}

impl InhibitSource for Logind {
    fn name(&self) -> &'static str {
        "logind"
    }

    fn fd(&self) -> RawFd {
        self.channel.watch().fd
    }

    fn is_inhibited(&self) -> bool {
        self.idle_blocked
    }

    /// Handles pending `PropertiesChanged(iface, changed, invalidated)` signals.
    fn dispatch(&mut self) -> Result<(), String> {
        for msg in bus::drain(&self.channel)? {
            if let Some(blocked) = block_inhibited(&msg) {
                self.update(&blocked);
            }
        }
        Ok(())
    }
}

/// Extracts the new `BlockInhibited` value from a `PropertiesChanged` signal.
/// logind sends the value itself (emits-change), not just an invalidation.
fn block_inhibited(msg: &Message) -> Option<String> {
    if msg.member().as_deref() != Some("PropertiesChanged") {
        return None;
    }
    let (_, changed) = msg.read2::<&str, PropMap>().ok()?;
    changed
        .get("BlockInhibited")?
        .0
        .as_str()
        .map(str::to_string)
}
