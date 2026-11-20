//! Everything Wayland: globals, the idle notification and the black overlay.
//!
//! The compositor tracks idle time and Wayland inhibitors (ext-idle-notify-v1).
//! On `idled` we map a layer-shell overlay showing a 1x1 black pixel stretched
//! with wp_viewporter; on `resumed` we destroy it.

use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::AsFd,
};

use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
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

use crate::logging::{INFO, WARNING, log};

/// The black overlay currently on screen, if any.
struct Overlay {
    surface: WlSurface,
    viewport: WpViewport,
    layer: ZwlrLayerSurfaceV1,
}

impl Overlay {
    /// Destroys the protocol objects. Order matters: layer-shell requires the
    /// role object (layer surface) to go before its `wl_surface`.
    fn destroy(self) {
        self.layer.destroy();
        self.viewport.destroy();
        self.surface.destroy();
    }
}

/// Global objects needed at runtime plus the current overlay.
/// Passed as `&mut` to every event handler (`Dispatch` impls below).
pub struct State {
    // Idle detection.
    notifier: ExtIdleNotifierV1,
    /// Idle time is tracked per seat; we use the first one advertised.
    seat: WlSeat,
    timeout_ms: u32,
    /// Active idle notification; `None` while a D-Bus inhibitor is held.
    notification: Option<ExtIdleNotificationV1>,

    // Drawing the overlay.
    compositor: WlCompositor,
    viewporter: WpViewporter,
    layer_shell: ZwlrLayerShellV1,
    /// The shared 1x1 black buffer, reused on every activation.
    buffer: WlBuffer,
    /// Obtained once the seat advertises pointer capability; used only to
    /// hide the cursor over the overlay.
    pointer: Option<WlPointer>,
    /// `Some` while the screen is blanked (or about to be, pending configure).
    overlay: Option<Overlay>,
}

/// Connects to the compositor and binds every required global.
/// The idle notification is not created yet: call `set_inhibited` for that.
pub fn connect(timeout_ms: u32) -> Result<(EventQueue<State>, State), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connection: {e}"))?;
    let (globals, queue) =
        registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();

    // Version ranges: we only use requests from v1 of every interface, so
    // v1 is the minimum. `wl_shm` is only needed to build the buffer.
    let shm: WlShm = bind(&globals, &qh, 1..=1)?;
    let state = State {
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
    Ok((queue, state))
}

/// Reads events from the socket once `poll` says it is readable.
/// `WouldBlock` (nothing to read after all, e.g. a spurious wakeup) is not
/// an error. Events are only queued here; `dispatch` runs their handlers.
pub fn read_events(guard: wayland_client::backend::ReadEventsGuard) -> Result<(), String> {
    match guard.read() {
        Ok(_) => Ok(()),
        Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(format!("Wayland read: {e}")),
    }
}

/// Runs the handlers for all Wayland events read so far.
pub fn dispatch(queue: &mut EventQueue<State>, state: &mut State) -> Result<(), String> {
    queue
        .dispatch_pending(state)
        .map(|_| ())
        .map_err(|e| format!("dispatch: {e}"))
}

impl State {
    /// Applies the D-Bus inhibition state. While inhibited there is no idle
    /// notification at all, so the compositor never tells us to blank; when
    /// released, a fresh notification restarts the idle countdown from zero.
    /// Idempotent: calling it again with the same value does nothing.
    pub fn set_inhibited(&mut self, inhibited: bool, qh: &QueueHandle<Self>) {
        if inhibited {
            if let Some(notification) = self.notification.take() {
                notification.destroy();
                // An inhibitor may arrive while blanked (e.g. a video started
                // remotely): with the notification gone no `resumed` would
                // ever come, so unblank now.
                self.hide();
            }
        } else if self.notification.is_none() {
            // v1 notification: respects Wayland idle inhibitors (Chrome, games...).
            let notification =
                self.notifier
                    .get_idle_notification(self.timeout_ms, &self.seat, qh, ());
            self.notification = Some(notification);
        }
    }

    /// Creates the overlay. Nothing is drawn yet: the compositor first replies
    /// with a `configure` (handled below), and only then is the buffer attached.
    fn show(&mut self, qh: &QueueHandle<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let surface = self.compositor.create_surface(qh, ());
        let viewport = self.viewporter.get_viewport(&surface, qh, ());
        // `None` output: the compositor picks the current monitor. The overlay
        // layer is stacked above everything, fullscreen windows included.
        // "wayvoid" is the namespace compositors may use in window rules.
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
}

/// Binds a required global, naming the missing interface on failure.
/// If the compositor advertises several (e.g. seats), the first one is used.
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
/// The memfd can be dropped on return: `create_pool` duplicates the fd when
/// queueing the request, and the compositor maps the memory on its side.
fn black_buffer(shm: &WlShm, qh: &QueueHandle<State>) -> Result<WlBuffer, String> {
    let fd = rustix::fs::memfd_create("wayvoid", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(|e| format!("memfd_create: {e}"))?;
    let mut file = File::from(fd);
    // ARGB8888 is a native-endian 32-bit value: alpha 0xFF, RGB all zero.
    file.write_all(&0xFF00_0000u32.to_ne_bytes())
        .map_err(|e| format!("write to memfd: {e}"))?;
    // Pool of 4 bytes; buffer at offset 0, 1x1 pixels, stride 4 bytes.
    let pool = shm.create_pool(file.as_fd(), 4, qh, ());
    let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
    // The buffer keeps the memory alive; the pool is no longer needed.
    pool.destroy();
    Ok(buffer)
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
/// `configure` arrives once after `show` and again if the output size changes.
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
                // The ack must precede the commit that applies the new size.
                overlay.layer.ack_configure(serial);
                // A 0 size would be a viewporter protocol error; -1 leaves the
                // destination unset. Never expected with all-edge anchoring.
                let size = |v: u32| if v == 0 { -1 } else { v as i32 };
                overlay.viewport.set_destination(size(width), size(height));
                overlay.surface.attach(Some(&state.buffer), 0, 0);
                overlay.surface.commit();
            }
            // The compositor dropped our surface (e.g. output gone). We stay
            // "idle" without an overlay; the next resume/idle cycle blanks again.
            zwlr_layer_surface_v1::Event::Closed => {
                log(WARNING, "overlay closed by the compositor");
                state.hide();
            }
            _ => {}
        }
    }
}

/// Request a pointer once the seat says it has one. Capabilities may be sent
/// again later (e.g. a mouse plugged in); we only ever create one pointer.
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

/// Hide the cursor whenever it enters our (only) surface. `enter` is only sent
/// for our own surfaces, and a `None` cursor surface means "no cursor"; other
/// clients' cursors are unaffected.
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

/// The registry is handled by `registry_queue_init`. Globals added or removed
/// later (hotplug) are ignored: we bind everything once at startup.
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

// Objects whose events we don't care about. Plain entries have no events at
// all (an unexpected one would panic); `ignore` ones have events we discard:
// `wl_shm.format`, `wl_buffer.release` and `wl_surface.enter`/`leave`.
delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ZwlrLayerShellV1);
delegate_noop!(State: ExtIdleNotifierV1);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
