//! `multimonitor` feature: track every `wl_output`, including hotplug.
//!
//! Outputs present at startup come from the initial global list; later ones
//! arrive as `wl_registry.global` / `global_remove` events. A monitor plugged
//! in while blanked is covered right away; an unplugged one loses its overlay.
//!
//! When the feature is off, `outputs_off.rs` replaces this file with the same
//! API: a single target (`None`, the compositor's choice) and no hotplug.

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::{
        wl_output::{self, WlOutput},
        wl_registry::{self, WlRegistry},
    },
};

use super::{State, theme};
use crate::logging::{INFO, log};

/// Highest `wl_output` version we bind: v3 adds `release` (to free the
/// object on unplug), v4 the `name` event (exported to `theme` children).
const MAX_VERSION: u32 = 4;

/// A monitor advertised by the compositor.
struct Output {
    /// Registry name of the global, to match `global_remove`.
    global: u32,
    wl: WlOutput,
    /// Connector name such as "HDMI-A-1" (wl_output v4); `None` until the
    /// `name` event arrives, or forever on older compositors.
    name: Option<String>,
}

/// This feature's part of `State`: every known output.
pub struct Outputs {
    list: Vec<Output>,
}

impl Outputs {
    /// Binds every output advertised at startup.
    pub fn bind_all(globals: &GlobalList, qh: &QueueHandle<State>) -> Self {
        let registry = globals.registry();
        let list = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == WlOutput::interface().name)
                .map(|g| bind(registry, g.name, g.version, qh))
                .collect()
        });
        Self { list }
    }

    /// The outputs to cover: all of them.
    pub fn targets(&self) -> Vec<Option<WlOutput>> {
        self.list.iter().map(|o| Some(o.wl.clone())).collect()
    }

    /// The name of `target` (e.g. "HDMI-A-1"), if known. Used by `theme`.
    #[cfg_attr(not(feature = "theme"), allow(dead_code))]
    pub fn name(&self, target: Option<&WlOutput>) -> Option<String> {
        self.list
            .iter()
            .find(|o| Some(&o.wl) == target)
            .and_then(|o| o.name.clone())
    }
}

fn bind(registry: &WlRegistry, global: u32, version: u32, qh: &QueueHandle<State>) -> Output {
    Output {
        global,
        wl: registry.bind(global, version.min(MAX_VERSION), qh, ()),
        name: None,
    }
}

/// From the registry handler: globals added or removed after startup (hotplug).
/// Only outputs matter; everything else was bound once at startup.
pub fn registry_event(
    state: &mut State,
    registry: &WlRegistry,
    event: wl_registry::Event,
    qh: &QueueHandle<State>,
) {
    match event {
        wl_registry::Event::Global {
            name,
            interface,
            version,
        } if interface == WlOutput::interface().name => {
            let output = bind(registry, name, version, qh);
            let wl = output.wl.clone();
            state.outputs.list.push(output);
            log(
                INFO,
                &format!("monitor connected ({} total)", state.outputs.list.len()),
            );
            // The theme (if any) starts once the output's name is known,
            // on its `done` event.
            if state.blanked {
                state.cover(Some(wl), qh);
            }
        }
        // `global_remove` carries only the name; it may be any global.
        wl_registry::Event::GlobalRemove { name } => {
            let list = &mut state.outputs.list;
            let Some(i) = list.iter().position(|o| o.global == name) else {
                return;
            };
            let output = list.swap_remove(i).wl;
            log(INFO, &format!("monitor disconnected ({} left)", list.len()));
            // The compositor usually sends `closed` too; removing here
            // covers compositors that don't.
            state.remove_overlays(|o| o.output.as_ref() == Some(&output));
            theme::stop(state, &output);
            if output.version() >= 3 {
                output.release();
            }
        }
        _ => {}
    }
}

/// Output events: only the name matters (layer-shell sizes the overlays for
/// us). On `done`, the output's information is complete: with `theme`, a
/// monitor plugged in while blanked gets its theme then. (`done` also comes
/// after mode changes; starting is a no-op if the theme is already running.)
impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        wl: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => {
                if let Some(output) = state.outputs.list.iter_mut().find(|o| o.wl == *wl) {
                    output.name = Some(name);
                }
            }
            wl_output::Event::Done => theme::ensure_started(state, wl.clone()),
            _ => {}
        }
    }
}
