//! D-Bus inhibitor sources: inhibitors the compositor doesn't see.
//!
//! - `screensaver`: the `org.freedesktop.ScreenSaver` service (Firefox...).
//! - `logind`: logind `idle` locks (`systemd-inhibit`).

mod bus;
mod logind;
mod screensaver;

use std::os::fd::RawFd;

use crate::logging::{WARNING, log};

/// A D-Bus connection that can hold the screensaver off.
/// The event loop polls `fd()` and calls `dispatch()` when it becomes readable.
pub trait InhibitSource {
    /// Human-readable name for log messages.
    fn name(&self) -> &'static str;
    /// The bus socket, to be polled for readability. Must stay valid for as
    /// long as the source exists: the event loop borrows it on every poll.
    fn fd(&self) -> RawFd;
    /// Whether this source currently wants the screen kept on.
    fn is_inhibited(&self) -> bool;
    /// Handles pending messages without blocking; fails if the bus is lost.
    /// May be called when nothing is pending: it must then do nothing.
    fn dispatch(&mut self) -> Result<(), String>;
}

/// Starts every source that can start. Each one is optional: on failure we
/// log a warning and carry on with the rest (and with Wayland inhibitors).
pub fn start() -> Vec<Box<dyn InhibitSource>> {
    let mut sources: Vec<Box<dyn InhibitSource>> = Vec::new();
    match screensaver::ScreenSaver::new() {
        Ok(src) => sources.push(Box::new(src)),
        Err(e) => warn_disabled(screensaver::SERVICE, &e),
    }
    match logind::Logind::new() {
        Ok(src) => sources.push(Box::new(src)),
        Err(e) => warn_disabled(logind::NAME, &e),
    }
    sources
}

/// Logged when a source can't start or loses its bus.
pub fn warn_disabled(source: &str, reason: &str) {
    log(WARNING, &format!("{source} inhibitors disabled: {reason}"));
}
