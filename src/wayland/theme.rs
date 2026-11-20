//! `command` feature, Wayland side: show an external program while blanked.
//!
//! The overlays stay on top of everything (they keep catching input, hiding
//! the cursor and swallowing the waking key), but show a transparent pixel
//! while their output's command is running, so the command's own fullscreen
//! window shows through. If the command isn't running (failed to start, or
//! exited too soon to be restarted), the overlay falls back to black.
//!
//! With `--backdrop`, each output also gets a black layer surface on the `top`
//! layer, below the command's fullscreen window, so the desktop isn't visible
//! while the command starts. This relies on the compositor stacking fullscreen
//! windows above the `top` layer, as wlroots-based ones and KWin do.
//!
//! The processes themselves are handled by `command::Runner`. When the
//! feature is off, `theme_off.rs` replaces this file with the same API, empty.

use std::os::fd::BorrowedFd;

use wayland_client::{
    QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm::WlShm},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::KeyboardInteractivity,
};

use super::{Overlay, State, pixel_buffer};
use crate::{cli::Config, command::Runner};

/// This feature's part of `State`.
pub struct Theme {
    /// `None` for `blank`: then everything behaves as without the feature.
    runner: Option<Runner>,
    backdrop: bool,
    /// Fully transparent 1x1 pixel, created only when there is a command.
    transparent: Option<WlBuffer>,
}

impl Theme {
    pub fn new(config: &Config, shm: &WlShm, qh: &QueueHandle<State>) -> Result<Self, String> {
        let runner = config.command.argv().map(Runner::new);
        let transparent = match runner {
            Some(_) => Some(pixel_buffer(shm, qh, 0x0000_0000)?),
            None => None,
        };
        Ok(Self {
            runner,
            backdrop: config.command.backdrop,
            transparent,
        })
    }
}

/// This feature's part of each `Overlay`.
#[derive(Default)]
pub struct OverlayExt {
    /// A `--backdrop` surface (always black, never takes the keyboard).
    backdrop: bool,
    /// Whether the first `configure` was handled; only then may a different
    /// buffer be attached (when the command starts or stops).
    configured: bool,
}

/// From the `configure` handler, once the overlay's first buffer is attached.
pub fn configured(overlay: &mut Overlay) {
    overlay.ext.configured = true;
}

/// The buffer an overlay should show, or `None` for the default black.
pub fn buffer_for<'a>(state: &'a State, overlay: &Overlay) -> Option<&'a WlBuffer> {
    let runner = state.theme.runner.as_ref()?;
    if overlay.ext.backdrop || !runner.is_running(overlay.output.as_ref()) {
        return None;
    }
    state.theme.transparent.as_ref()
}

/// With `--backdrop` and a command, adds the black surface below `output`'s
/// command window.
pub fn add_backdrop(state: &mut State, output: Option<WlOutput>, qh: &QueueHandle<State>) {
    if state.theme.backdrop && state.theme.runner.is_some() {
        // No keyboard: the overlay above keeps it.
        let mut overlay = state.new_overlay(output, Layer::Top, KeyboardInteractivity::None, qh);
        overlay.ext.backdrop = true;
        state.overlays.push(overlay);
    }
}

/// Starts the command on every covered output (right after blanking).
pub fn start_all(state: &mut State) {
    if state.theme.runner.is_none() {
        return;
    }
    for target in state.outputs.targets() {
        start(state, target);
    }
}

/// Starts the command for `target` if blanked and not already running there.
/// Used for outputs plugged in while blanked, once their name is known.
#[cfg_attr(not(feature = "multimonitor"), allow(dead_code))]
pub fn ensure_started(state: &mut State, target: WlOutput) {
    let running = match &state.theme.runner {
        Some(runner) => runner.is_running(Some(&target)),
        None => return,
    };
    if state.blanked && !running {
        start(state, Some(target));
    }
}

/// Starts one child and updates its output's overlay (transparent if it runs).
fn start(state: &mut State, target: Option<WlOutput>) {
    let name = state.outputs.name(target.as_ref());
    if let Some(runner) = &mut state.theme.runner {
        runner.start(target.clone(), name.as_deref());
    }
    refresh(state, target.as_ref());
}

/// Stops every child (when unblanking).
pub fn stop_all(state: &mut State) {
    if let Some(runner) = &mut state.theme.runner {
        runner.stop_all();
    }
}

/// Stops the child of an unplugged output.
#[cfg_attr(not(feature = "multimonitor"), allow(dead_code))]
pub fn stop(state: &mut State, output: &WlOutput) {
    if let Some(runner) = &mut state.theme.runner {
        runner.stop(output);
    }
}

/// Children's pidfds, for the event loop to poll.
pub fn fds(state: &State) -> Vec<BorrowedFd<'_>> {
    match &state.theme.runner {
        Some(runner) => runner.fds().collect(),
        None => Vec::new(),
    }
}

/// Handles children that exited: restarts the ones that lived long enough,
/// and turns the overlay black where the command is no longer running.
pub fn reap(state: &mut State) {
    let Some(runner) = &mut state.theme.runner else {
        return;
    };
    let exited_children = runner.reap();
    for exited in exited_children {
        // Only live children are reported, and those only exist while blanked.
        if exited.restart {
            start(state, exited.target);
        } else {
            refresh(state, exited.target.as_ref());
        }
    }
}

/// Re-attaches the right buffer to the configured overlays of `target`,
/// after its command started or stopped.
fn refresh(state: &State, target: Option<&WlOutput>) {
    for overlay in &state.overlays {
        if overlay.ext.configured && overlay.output.as_ref() == target {
            overlay
                .surface
                .attach(Some(state.overlay_buffer(overlay)), 0, 0);
            // A new buffer must be damaged for the compositor to redraw.
            overlay.surface.damage(0, 0, i32::MAX, i32::MAX);
            overlay.surface.commit();
        }
    }
}
