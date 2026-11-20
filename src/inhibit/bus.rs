//! Small helpers shared by the D-Bus inhibitor sources.

use std::time::Duration;

use dbus::{
    Message,
    channel::{BusType, Channel},
};

/// The bus daemon itself, for `RequestName`, `AddMatch`...
pub const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
/// Timeout for the blocking calls, all made at startup: a stuck bus delays
/// startup by at most this much per call, then that source is disabled.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens a private connection with fd watching enabled, so the socket
/// can be polled in the event loop.
pub fn connect(bus: BusType) -> Result<Channel, String> {
    // Private: not shared with any other libdbus user in the process.
    // Blocks briefly for the connection and the bus `Hello` handshake.
    let mut channel = Channel::get_private(bus).map_err(|e| format!("{bus:?} bus: {e}"))?;
    // Required by `Channel::watch()`, which panics otherwise.
    channel.set_watch_enabled(true);
    Ok(channel)
}

/// Blocking method call to `(destination, object path, interface)`;
/// `args` appends the call's arguments. Only used at startup. While it waits,
/// unrelated incoming messages stay queued for the next `drain`.
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
    // Zero timeout: read (and write) whatever is ready, never wait.
    channel
        .read_write(Some(Duration::ZERO))
        .map_err(|()| "bus connection lost".to_string())?;
    Ok(std::iter::from_fn(|| channel.pop_message()).collect())
}
