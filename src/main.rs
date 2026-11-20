//! wayvoid: a minimal Wayland screensaver.
//!
//! After N seconds of inactivity it covers the screen with a black
//! layer-shell surface, and removes it as soon as activity resumes.
//! The compositor tracks idle time and Wayland inhibitors (ext-idle-notify-v1).
//! Inhibitors the compositor doesn't see come from D-Bus sources:
//! - `screensaver.rs`: the `org.freedesktop.ScreenSaver` service (Firefox...).
//! - `logind.rs`: logind `idle` locks (`systemd-inhibit`).
//!
//! A single thread blocks in poll() on all sockets: 0% CPU while idle.

mod bus;
mod logind;
mod screensaver;

use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsFd, BorrowedFd},
    process::ExitCode,
    sync::OnceLock,
};

use bus::InhibitSource;
use logind::Logind;
use rustix::event::{PollFd, PollFlags, poll};
use screensaver::ScreenSaver;

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    backend::WaylandError,
    delegate_noop,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_pointer::{self, WlPointer},
        wl_registry::WlRegistry,
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::{
    ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self, ExtIdleNotificationV1},
        ext_idle_notifier_v1::ExtIdleNotifierV1,
    },
    wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

// --- Logging -----------------------------------------------------------------

/// syslog priorities understood by journald as `<N>` line prefixes.
const ERR: u8 = 3;
const WARNING: u8 = 4;
const INFO: u8 = 6;

/// True when stderr is connected to the systemd journal.
/// systemd exports `JOURNAL_STREAM=<dev>:<inode>` for the stream it hands us;
/// comparing it with stderr avoids false positives from inherited variables.
fn stderr_is_journal() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let Ok(var) = std::env::var("JOURNAL_STREAM") else {
            return false;
        };
        let Ok(stat) = rustix::fs::fstat(std::io::stderr()) else {
            return false;
        };
        var == format!("{}:{}", stat.st_dev, stat.st_ino)
    })
}

/// Writes one log line to stderr.
/// In the journal: `<N>msg`, so the priority is kept and filterable (`-p`).
/// In a terminal: `wayvoid: msg`.
fn log(priority: u8, msg: &str) {
    if stderr_is_journal() {
        eprintln!("<{priority}>{msg}");
    } else {
        eprintln!("wayvoid: {msg}");
    }
}

// --- State --------------------------------------------------------------------

/// The black overlay currently on screen, if any.
struct Overlay {
    surface: WlSurface,
    viewport: WpViewport,
    layer: ZwlrLayerSurfaceV1,
}

impl Overlay {
    /// Destroys the protocol objects (role object first, surface last).
    fn destroy(self) {
        self.layer.destroy();
        self.viewport.destroy();
        self.surface.destroy();
    }
}

/// Global objects needed at runtime plus the current overlay.
struct State {
    notifier: ExtIdleNotifierV1,
    seat: WlSeat,
    timeout_ms: u32,
    /// Active idle notification; `None` while a D-Bus inhibitor is held.
    notification: Option<ExtIdleNotificationV1>,
    compositor: WlCompositor,
    viewporter: WpViewporter,
    layer_shell: ZwlrLayerShellV1,
    /// The shared 1x1 black buffer, reused on every activation.
    buffer: WlBuffer,
    /// Obtained once the seat advertises pointer capability.
    pointer: Option<WlPointer>,
    overlay: Option<Overlay>,
}

impl State {
    /// Creates the overlay. It becomes visible on the first `configure`.
    fn show(&mut self, qh: &QueueHandle<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let surface = self.compositor.create_surface(qh, ());
        let viewport = self.viewporter.get_viewport(&surface, qh, ());
        // `None` output: the compositor picks the current monitor.
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            None,
            Layer::Overlay,
            "wayvoid".into(),
            qh,
            (),
        );
        // Anchored to all edges with size 0x0 means "fill the output".
        layer.set_anchor(Anchor::all());
        // -1: cover panels and bars instead of being pushed by them.
        layer.set_exclusive_zone(-1);
        // Grab the keyboard so the waking keypress reaches no application.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        // Initial commit without a buffer asks the compositor to configure us.
        surface.commit();
        self.overlay = Some(Overlay {
            surface,
            viewport,
            layer,
        });
    }

    /// Removes the overlay, if present.
    fn hide(&mut self) {
        if let Some(overlay) = self.overlay.take() {
            overlay.destroy();
        }
    }

    /// Applies the D-Bus inhibition state. While inhibited there is no idle
    /// notification at all, so the compositor never tells us to blank; when
    /// released, a fresh notification restarts the idle countdown from zero.
    fn set_inhibited(&mut self, inhibited: bool, qh: &QueueHandle<Self>) {
        if inhibited {
            if let Some(notification) = self.notification.take() {
                notification.destroy();
                self.hide();
            }
        } else if self.notification.is_none() {
            // v1 notification: respects Wayland idle inhibitors (Chrome, games...).
            self.notification =
                Some(
                    self.notifier
                        .get_idle_notification(self.timeout_ms, &self.seat, qh, ()),
                );
        }
    }
}

/// Binds a required global, naming the missing interface on failure.
fn bind<I>(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    versions: std::ops::RangeInclusive<u32>,
) -> Result<I, String>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    globals
        .bind(qh, versions, ())
        .map_err(|e| format!("{}: {e}", I::interface().name))
}

/// Creates a 1x1 ARGB8888 buffer holding a single opaque black pixel.
/// The memfd can be dropped afterwards: the compositor keeps its own copy of the fd.
fn black_buffer(shm: &WlShm, qh: &QueueHandle<State>) -> Result<WlBuffer, String> {
    let fd = rustix::fs::memfd_create("wayvoid", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(|e| format!("memfd_create: {e}"))?;
    let mut file = File::from(fd);
    // ARGB8888 is a native-endian 32-bit value: alpha 0xFF, RGB all zero.
    file.write_all(&0xFF00_0000u32.to_ne_bytes())
        .map_err(|e| format!("write to memfd: {e}"))?;
    let pool = shm.create_pool(file.as_fd(), 4, qh, ());
    let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
    // The buffer keeps the memory alive; the pool is no longer needed.
    pool.destroy();
    Ok(buffer)
}

/// Parses the single required argument: idle timeout in seconds (> 0).
/// Returns it in milliseconds, as ext-idle-notify expects.
fn parse_timeout_ms() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    match (args.next(), args.next()) {
        (Some(arg), None) => arg
            .parse::<u32>()
            .ok()
            .filter(|&s| s > 0)
            .and_then(|s| s.checked_mul(1000)),
        _ => None,
    }
}

fn run(timeout_ms: u32) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connection: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();

    let shm: WlShm = bind(&globals, &qh, 1..=1)?;

    let mut state = State {
        notifier: bind(&globals, &qh, 1..=1)?,
        seat: bind(&globals, &qh, 1..=1)?,
        timeout_ms,
        notification: None,
        compositor: bind(&globals, &qh, 1..=4)?,
        viewporter: bind(&globals, &qh, 1..=1)?,
        // v1 is enough: `Exclusive` (1) matches the old boolean "true".
        layer_shell: bind(&globals, &qh, 1..=4)?,
        buffer: black_buffer(&shm, &qh)?,
        pointer: None,
        overlay: None,
    };

    // Each D-Bus source is optional: if it can't start we log a warning
    // and keep working with the rest (and with Wayland inhibitors).
    let mut sources: Vec<Box<dyn InhibitSource>> = Vec::new();
    match ScreenSaver::new() {
        Ok(src) => sources.push(Box::new(src)),
        Err(e) => warn_disabled("org.freedesktop.ScreenSaver", &e),
    }
    match Logind::new() {
        Ok(src) => sources.push(Box::new(src)),
        Err(e) => warn_disabled("logind", &e),
    }

    state.set_inhibited(sources.iter().any(|s| s.is_inhibited()), &qh);
    log(
        INFO,
        &format!("started, idle timeout {}s", timeout_ms / 1000),
    );

    // Which sources have data to read. All start "ready" because messages
    // may already be buffered from the startup calls.
    let mut ready = vec![true; sources.len()];
    loop {
        if ready.contains(&true) {
            // Dispatch the ready sources, dropping any whose bus was lost.
            let mut i = 0;
            sources.retain_mut(|src| {
                let keep = !ready[i]
                    || src
                        .dispatch()
                        .inspect_err(|e| warn_disabled(src.name(), e))
                        .is_ok();
                i += 1;
                keep
            });
            ready.clear();
            state.set_inhibited(sources.iter().any(|s| s.is_inhibited()), &qh);
        }

        // Send our requests, then block until any socket has data.
        queue.flush().map_err(|e| format!("flush: {e}"))?;
        // `prepare_read` returns None if events are already queued.
        let Some(guard) = queue.prepare_read() else {
            dispatch(&mut queue, &mut state)?;
            continue;
        };
        let wl_ready;
        {
            let wl_fd = guard.connection_fd();
            // SAFETY: each fd belongs to a source's channel, which outlives this block.
            let bus_fds: Vec<_> = sources
                .iter()
                .map(|src| unsafe { BorrowedFd::borrow_raw(src.fd()) })
                .collect();
            let mut fds = vec![PollFd::new(&wl_fd, PollFlags::IN)];
            fds.extend(bus_fds.iter().map(|fd| PollFd::new(fd, PollFlags::IN)));
            match poll(&mut fds, None) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(format!("poll: {e}")),
            }
            // Any revents (IN, HUP, ERR) means "go read": errors surface there.
            wl_ready = !fds[0].revents().is_empty();
            ready = fds[1..].iter().map(|fd| !fd.revents().is_empty()).collect();
        }

        if wl_ready {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Wayland read: {e}")),
            }
        }
        // (Dropping an unused guard cancels the read.)
        dispatch(&mut queue, &mut state)?;
    }
}

/// Logged when a D-Bus inhibitor source can't start or loses its bus.
fn warn_disabled(source: &str, reason: &str) {
    log(WARNING, &format!("{source} inhibitors disabled: {reason}"));
}

/// Runs the handlers for all Wayland events read so far.
fn dispatch(
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
) -> Result<(), String> {
    queue
        .dispatch_pending(state)
        .map(|_| ())
        .map_err(|e| format!("dispatch: {e}"))
}

fn main() -> ExitCode {
    let Some(timeout_ms) = parse_timeout_ms() else {
        log(ERR, "usage: wayvoid <idle-seconds>");
        return ExitCode::from(2);
    };
    match run(timeout_ms) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log(ERR, &e);
            ExitCode::FAILURE
        }
    }
}

// --- Event handlers ---------------------------------------------------------

/// Idle state changes drive the whole program.
impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                log(INFO, "idle: screen blanked");
                state.show(qh);
            }
            ext_idle_notification_v1::Event::Resumed => {
                log(INFO, "activity resumed: screen restored");
                state.hide();
            }
            _ => {}
        }
    }
}

/// Layer-shell handshake: ack the size, then stretch the 1x1 pixel over it.
impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Ignore events from a previous, already destroyed overlay.
        let Some(overlay) = state.overlay.as_ref().filter(|o| o.layer == *layer) else {
            return;
        };
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                overlay.layer.ack_configure(serial);
                overlay
                    .viewport
                    .set_destination(width as i32, height as i32);
                overlay.surface.attach(Some(&state.buffer), 0, 0);
                overlay.surface.commit();
            }
            // The compositor dropped our surface (e.g. output gone).
            zwlr_layer_surface_v1::Event::Closed => {
                log(WARNING, "overlay closed by the compositor");
                state.hide();
            }
            _ => {}
        }
    }
}

/// Request a pointer once the seat says it has one.
impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

/// Hide the cursor whenever it enters our (only) surface.
impl Dispatch<WlPointer, ()> for State {
    fn event(
        _: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Enter { serial, .. } = event {
            pointer.set_cursor(serial, None, 0, 0);
        }
    }
}

/// The registry is handled by `registry_queue_init`; nothing to do here.
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Objects whose events we don't care about (or that have none).
delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ZwlrLayerShellV1);
delegate_noop!(State: ExtIdleNotifierV1);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
