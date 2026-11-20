//! Everything Wayland: globals, the idle notification and the black overlay.
//!
//! The compositor tracks idle time and Wayland inhibitors (ext-idle-notify-v1).
//! On `idled` we map a layer-shell overlay showing a 1x1 black pixel stretched
//! with wp_viewporter; on `resumed` we destroy it.
//!
//! Without the `multimonitor` feature there is a single overlay with no
//! output set (the compositor picks one). With it, `outputs` tracks every
//! `wl_output` and each one gets its own overlay.
//!
//! With the `command` feature (`theme.rs`), the overlays can turn transparent
//! to show an external program running underneath.

#[cfg(feature = "ignore-mouse")]
mod input;
#[cfg(feature = "multimonitor")]
mod outputs;
#[cfg(feature = "command")]
mod theme;

use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::AsFd,
};

use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::WaylandError,
    delegate_noop,
    globals::{GlobalList, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_output::WlOutput,
        wl_pointer::{self, WlPointer},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
// Only for the static registry handler; `multimonitor` has its own.
#[cfg(not(feature = "multimonitor"))]
use wayland_client::{globals::GlobalListContents, protocol::wl_registry::WlRegistry};
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

use crate::cli::Config;
use crate::logging::{INFO, WARNING, log};

/// A layer surface covering one output: normally the black overlay on top
/// of everything; with `command --backdrop`, also a black backdrop below the
/// command's window.
struct Overlay {
    /// The output it covers, to find the overlay when that output is unplugged.
    #[cfg(feature = "multimonitor")]
    output: Option<WlOutput>,
    surface: WlSurface,
    viewport: WpViewport,
    layer: ZwlrLayerSurfaceV1,
    /// A `--backdrop` surface (always black, never takes the keyboard).
    #[cfg(feature = "command")]
    backdrop: bool,
    /// Whether the first `configure` was handled; only then may a different
    /// buffer be attached (when the command starts or stops).
    #[cfg(feature = "command")]
    configured: bool,
}

impl Overlay {
    /// The output this overlay was created for (`None`: compositor's choice).
    #[cfg(feature = "command")]
    fn target(&self) -> Option<WlOutput> {
        #[cfg(feature = "multimonitor")]
        return self.output.clone();
        #[cfg(not(feature = "multimonitor"))]
        None
    }

    /// Destroys the protocol objects. Order matters: layer-shell requires the
    /// role object (layer surface) to go before its `wl_surface`.
    fn destroy(self) {
        self.layer.destroy();
        self.viewport.destroy();
        self.surface.destroy();
    }
}

/// Global objects needed at runtime plus the current overlays.
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
    /// True between `idled` and `resumed` (or an inhibitor arriving).
    blanked: bool,
    /// One per covered output while blanked; empty otherwise. May also be
    /// empty while blanked if the compositor closed them all.
    overlays: Vec<Overlay>,
    /// Every advertised output (for hotplug and, with `command`, its name).
    #[cfg(feature = "multimonitor")]
    outputs: Vec<outputs::Output>,
    /// `--ignore-mouse` state and the extra input objects it needs.
    #[cfg(feature = "ignore-mouse")]
    ignore_mouse: input::IgnoreMouse,
    /// The external command, its children and the transparent buffer.
    #[cfg(feature = "command")]
    theme: theme::Theme,
}

/// Connects to the compositor and binds every required global.
/// The idle notification is not created yet: call `set_inhibited` for that.
pub fn connect(config: &Config) -> Result<(EventQueue<State>, State), String> {
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
        timeout_ms: config.timeout_ms,
        notification: None,
        compositor: bind(&globals, &qh, 1..=4)?,
        viewporter: bind(&globals, &qh, 1..=1)?,
        // v1 is enough: `Exclusive` (1) matches the old boolean "true".
        layer_shell: bind(&globals, &qh, 1..=4)?,
        // ARGB8888: alpha 0xFF, RGB all zero.
        buffer: pixel_buffer(&shm, &qh, 0xFF00_0000)?,
        pointer: None,
        blanked: false,
        overlays: Vec::new(),
        #[cfg(feature = "multimonitor")]
        outputs: outputs::bind_all(&globals, &qh),
        #[cfg(feature = "ignore-mouse")]
        ignore_mouse: input::IgnoreMouse::new(config.ignore_mouse),
        #[cfg(feature = "command")]
        theme: theme::Theme::new(config, &shm, &qh)?,
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

    /// Children's pidfds (`command`), to be polled by the event loop.
    #[cfg(feature = "command")]
    pub fn command_fds(&self) -> Vec<std::os::fd::BorrowedFd<'_>> {
        theme::fds(self)
    }

    /// Called by the event loop when a child's pidfd is readable.
    #[cfg(feature = "command")]
    pub fn command_exited(&mut self) {
        theme::reap(self);
    }

    /// Blanks the screen: covers every target output.
    fn show(&mut self, qh: &QueueHandle<Self>) {
        if self.blanked {
            return;
        }
        self.blanked = true;
        for output in self.targets() {
            self.cover(output, qh);
        }
        #[cfg(feature = "command")]
        theme::start_all(self);
    }

    /// Unblanks: destroys every overlay (and stops the command).
    fn hide(&mut self) {
        self.blanked = false;
        for overlay in self.overlays.drain(..) {
            overlay.destroy();
        }
        #[cfg(feature = "command")]
        theme::stop_all(self);
    }

    /// Covers one output: its overlay (plus a backdrop with `command`).
    fn cover(&mut self, output: Option<WlOutput>, qh: &QueueHandle<Self>) {
        #[cfg(feature = "command")]
        theme::add_backdrop(self, output.clone(), qh);
        self.add_overlay(output, qh);
    }

    /// The outputs to cover: all known ones with `multimonitor`, otherwise a
    /// single `None` ("whichever output the compositor picks").
    fn targets(&self) -> Vec<Option<WlOutput>> {
        #[cfg(feature = "multimonitor")]
        return self.outputs.iter().map(|o| Some(o.wl.clone())).collect();
        #[cfg(not(feature = "multimonitor"))]
        vec![None]
    }

    /// Adds the overlay for `output`: on the `overlay` layer, stacked above
    /// everything (fullscreen windows included), with the keyboard grabbed so
    /// the waking keypress reaches no application. With several overlays the
    /// compositor focuses one of them.
    fn add_overlay(&mut self, output: Option<WlOutput>, qh: &QueueHandle<Self>) {
        let overlay =
            self.new_overlay(output, Layer::Overlay, KeyboardInteractivity::Exclusive, qh);
        self.overlays.push(overlay);
    }

    /// Creates a full-output layer surface. Nothing is drawn yet: the
    /// compositor first replies with a `configure` (handled below), and only
    /// then is the buffer attached.
    fn new_overlay(
        &self,
        output: Option<WlOutput>,
        layer: Layer,
        keyboard: KeyboardInteractivity,
        qh: &QueueHandle<Self>,
    ) -> Overlay {
        let surface = self.compositor.create_surface(qh, ());
        let viewport = self.viewporter.get_viewport(&surface, qh, ());
        // "wayvoid" is the namespace compositors may use in window rules.
        let layer_surface = self.layer_shell.get_layer_surface(
            &surface,
            output.as_ref(),
            layer,
            "wayvoid".into(),
            qh,
            (),
        );
        // Anchored to all edges with size 0x0 means "fill the output".
        layer_surface.set_anchor(Anchor::all());
        // -1: cover panels and bars instead of being pushed by them.
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(keyboard);
        // Initial commit without a buffer asks the compositor to configure us.
        surface.commit();
        Overlay {
            #[cfg(feature = "multimonitor")]
            output,
            surface,
            viewport,
            layer: layer_surface,
            #[cfg(feature = "command")]
            backdrop: false,
            #[cfg(feature = "command")]
            configured: false,
        }
    }

    /// The buffer an overlay shows: black, unless `command` says otherwise.
    fn overlay_buffer(&self, overlay: &Overlay) -> &WlBuffer {
        #[cfg(feature = "command")]
        if let Some(buffer) = theme::buffer_for(self, overlay) {
            return buffer;
        }
        #[cfg(not(feature = "command"))]
        let _ = overlay;
        &self.buffer
    }

    /// Re-attaches the right buffer to the configured overlays of `target`,
    /// after its command started or stopped.
    #[cfg(feature = "command")]
    fn refresh_overlays(&self, target: Option<&WlOutput>) {
        for overlay in &self.overlays {
            if overlay.configured && overlay.target().as_ref() == target {
                overlay
                    .surface
                    .attach(Some(self.overlay_buffer(overlay)), 0, 0);
                // A new buffer must be damaged for the compositor to redraw.
                overlay.surface.damage(0, 0, i32::MAX, i32::MAX);
                overlay.surface.commit();
            }
        }
    }

    /// The name of `target` (e.g. "HDMI-A-1"), exported to the command.
    #[cfg(feature = "command")]
    fn output_name(&self, target: Option<&WlOutput>) -> Option<String> {
        #[cfg(feature = "multimonitor")]
        return self
            .outputs
            .iter()
            .find(|o| Some(&o.wl) == target)
            .and_then(|o| o.name.clone());
        #[cfg(not(feature = "multimonitor"))]
        {
            let _ = target;
            None
        }
    }

    /// Destroys the overlays matching `pred` (blanked state is unchanged).
    fn remove_overlays(&mut self, pred: impl Fn(&Overlay) -> bool) {
        let mut i = 0;
        while i < self.overlays.len() {
            if pred(&self.overlays[i]) {
                self.overlays.swap_remove(i).destroy();
            } else {
                i += 1;
            }
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

/// Creates a 1x1 ARGB8888 buffer holding a single `argb` pixel.
/// The memfd can be dropped on return: `create_pool` duplicates the fd when
/// queueing the request, and the compositor maps the memory on its side.
fn pixel_buffer(shm: &WlShm, qh: &QueueHandle<State>, argb: u32) -> Result<WlBuffer, String> {
    let fd = rustix::fs::memfd_create("wayvoid", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(|e| format!("memfd_create: {e}"))?;
    let mut file = File::from(fd);
    // ARGB8888 is a native-endian 32-bit value.
    file.write_all(&argb.to_ne_bytes())
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
                // Logged only on an actual change: with `--ignore-mouse` we
                // may still be blanked from a previous `idled`.
                if !state.blanked {
                    log(INFO, "idle: screen blanked");
                }
                state.show(qh);
            }
            ext_idle_notification_v1::Event::Resumed => {
                // `--ignore-mouse`: the overlays decide when to wake (see
                // input.rs). Unless none is left (all closed by the compositor):
                // then nothing could receive that input, so wake now.
                #[cfg(feature = "ignore-mouse")]
                if state.ignore_mouse.enabled && !state.overlays.is_empty() {
                    return;
                }
                if state.blanked {
                    log(INFO, "activity resumed: screen restored");
                }
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
        // Ignore events from an already destroyed overlay.
        let Some(i) = state.overlays.iter().position(|o| o.layer == *layer) else {
            return;
        };
        let overlay = &state.overlays[i];
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
                overlay
                    .surface
                    .attach(Some(state.overlay_buffer(overlay)), 0, 0);
                overlay.surface.commit();
                #[cfg(feature = "command")]
                {
                    state.overlays[i].configured = true;
                }
            }
            // The compositor dropped this surface (e.g. its output is gone).
            // Only this overlay goes; we stay blanked, and the next
            // resume/idle cycle recreates whatever is missing.
            zwlr_layer_surface_v1::Event::Closed => {
                log(WARNING, "overlay closed by the compositor");
                state.remove_overlays(|o| o.layer == *layer);
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
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
        #[cfg(feature = "ignore-mouse")]
        input::seat_capabilities(state, seat, caps, qh);
    }
}

/// Hide the cursor whenever it enters one of our surfaces. `enter` is only sent
/// for our own surfaces, and a `None` cursor surface means "no cursor"; other
/// clients' cursors are unaffected. With `ignore-mouse`, the other pointer
/// events go to `input::pointer_event`.
impl Dispatch<WlPointer, ()> for State {
    #[cfg_attr(not(feature = "ignore-mouse"), allow(unused_variables))]
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, .. } => pointer.set_cursor(serial, None, 0, 0),
            #[cfg(feature = "ignore-mouse")]
            event => input::pointer_event(state, event),
            #[cfg(not(feature = "ignore-mouse"))]
            _ => {}
        }
    }
}

/// The registry is handled by `registry_queue_init`. Globals added or removed
/// later (hotplug) are ignored: we bind everything once at startup.
/// (`multimonitor` replaces this with its own handler, in `outputs.rs`.)
#[cfg(not(feature = "multimonitor"))]
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
