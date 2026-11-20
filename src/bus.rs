//! Small helpers shared by the D-Bus inhibitor sources.

use std::{os::fd::RawFd, time::Duration};

use dbus::{
    Message,
    channel::{BusType, Channel},
};

/// The bus daemon itself, for `RequestName`, `AddMatch`...
pub const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
/// Timeout for the few blocking calls (only made at startup).
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens a private connection with fd watching enabled, so the socket
/// can be polled in main's event loop.
pub fn connect(bus: BusType) -> Result<Channel, String> {
    let mut channel = Channel::get_private(bus).map_err(|e| format!("{bus:?} bus: {e}"))?;
    channel.set_watch_enabled(true);
    Ok(channel)
}

/// Blocking method call; `args` appends the call's arguments.
pub fn call(
    channel: &Channel,
    (dest, path, iface): (&str, &str, &str),
    method: &str,
    args: impl FnOnce(Message) -> Message,
) -> Result<Message, String> {
    let msg = Message::new_method_call(dest, path, iface, method)?;
    channel
        .send_with_reply_and_block(args(msg), CALL_TIMEOUT)
        .map_err(|e| format!("{method}: {e}"))
}

/// Blocking call to the bus daemon.
pub fn call_daemon(
    channel: &Channel,
    method: &str,
    args: impl FnOnce(Message) -> Message,
) -> Result<Message, String> {
    call(channel, (DBUS, DBUS_PATH, DBUS), method, args)
}

/// Reads everything available on the socket without blocking and returns the
/// queued messages. Fails if the connection is lost.
pub fn drain(channel: &Channel) -> Result<Vec<Message>, String> {
    channel
        .read_write(Some(Duration::ZERO))
        .map_err(|()| "bus connection lost".to_string())?;
    Ok(std::iter::from_fn(|| channel.pop_message()).collect())
}

/// A D-Bus connection that can hold the screensaver off.
/// main polls `fd()` and calls `dispatch()` when it becomes readable.
pub trait InhibitSource {
    /// Human-readable name for log messages.
    fn name(&self) -> &'static str;
    /// The bus socket, to be polled for readability.
    fn fd(&self) -> RawFd;
    /// Whether this source currently wants the screen kept on.
    fn is_inhibited(&self) -> bool;
    /// Handles pending messages without blocking; fails if the bus is lost.
    fn dispatch(&mut self) -> Result<(), String>;
}
