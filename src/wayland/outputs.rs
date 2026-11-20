//! `multimonitor` feature: track every `wl_output`, including hotplug.
//!
//! Outputs present at startup come from the initial global list; later ones
//! arrive as `wl_registry.global` / `global_remove` events. A monitor plugged
//! in while blanked is covered right away; an unplugged one loses its overlay.

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalList, GlobalListContents},
    protocol::{
        wl_output::{self, WlOutput},
        wl_registry::{self, WlRegistry},
    },
};

use super::State;
use crate::logging::{INFO, log};

/// Highest `wl_output` version we bind: v3 adds `release` (to free the
/// object on unplug), v4 the `name` event (exported to `command` children).
const MAX_VERSION: u32 = 4;

/// A monitor advertised by the compositor.
pub struct Output {
    /// Registry name of the global, to match `global_remove`.
    pub global: u32,
    pub wl: WlOutput,
    /// Connector name such as "HDMI-A-1" (wl_output v4); `None` until the
    /// `name` event arrives, or forever on older compositors.
    pub name: Option<String>,
}

/// Binds every output advertised at startup.
pub fn bind_all(globals: &GlobalList, qh: &QueueHandle<State>) -> Vec<Output> {
    let registry = globals.registry();
    globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| bind(registry, g.name, g.version, qh))
            .collect()
    })
}

fn bind(registry: &WlRegistry, global: u32, version: u32, qh: &QueueHandle<State>) -> Output {
    Output {
        global,
        wl: registry.bind(global, version.min(MAX_VERSION), qh, ()),
        name: None,
    }
}

/// Hotplug: globals added or removed after startup. Only outputs matter;
/// everything else was bound once at startup.
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                let output = bind(registry, name, version, qh);
                let wl = output.wl.clone();
                state.outputs.push(output);
                log(
                    INFO,
                    &format!("monitor connected ({} total)", state.outputs.len()),
                );
                // The command (if any) starts once the output's name is known,
                // on its `done` event.
                if state.blanked {
                    state.cover(Some(wl), qh);
                }
            }
            // `global_remove` carries only the name; it may be any global.
            wl_registry::Event::GlobalRemove { name } => {
                let Some(i) = state.outputs.iter().position(|o| o.global == name) else {
                    return;
                };
                let output = state.outputs.swap_remove(i).wl;
                log(
                    INFO,
                    &format!("monitor disconnected ({} left)", state.outputs.len()),
                );
                // The compositor usually sends `closed` too; removing here
                // covers compositors that don't.
                state.remove_overlays(|o| o.output.as_ref() == Some(&output));
                #[cfg(feature = "command")]
                super::theme::stop(state, &output);
                if output.version() >= 3 {
                    output.release();
                }
            }
            _ => {}
        }
    }
}

/// Output events: only the name matters (layer-shell sizes the overlays for
/// us). On `done`, the output's information is complete: with `command`, a
/// monitor plugged in while blanked gets its command then. (`done` also comes
/// after mode changes; starting is a no-op if the command is already running.)
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
                if let Some(output) = state.outputs.iter_mut().find(|o| o.wl == *wl) {
                    output.name = Some(name);
                }
            }
            #[cfg(feature = "command")]
            wl_output::Event::Done => super::theme::ensure_started(state, wl.clone()),
            _ => {}
        }
    }
}
