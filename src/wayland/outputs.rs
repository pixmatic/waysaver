//! `multimonitor` feature: track every `wl_output`, including hotplug.
//!
//! Outputs present at startup come from the initial global list; later ones
//! arrive as `wl_registry.global` / `global_remove` events. A monitor plugged
//! in while blanked is covered right away; an unplugged one loses its overlay.

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalList, GlobalListContents},
    protocol::{
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
    },
};

use super::State;
use crate::logging::{INFO, log};

/// Highest `wl_output` version we bind. We use no output requests or events;
/// v3 only adds `release`, which lets us free the object on unplug.
const MAX_VERSION: u32 = 3;

/// Binds every output advertised at startup.
pub fn bind_all(globals: &GlobalList, qh: &QueueHandle<State>) -> Vec<(u32, WlOutput)> {
    let registry = globals.registry();
    globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| (g.name, bind(registry, g.name, g.version, qh)))
            .collect()
    })
}

fn bind(registry: &WlRegistry, name: u32, version: u32, qh: &QueueHandle<State>) -> WlOutput {
    registry.bind(name, version.min(MAX_VERSION), qh, ())
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
                state.outputs.push((name, output.clone()));
                log(
                    INFO,
                    &format!("monitor connected ({} total)", state.outputs.len()),
                );
                if state.blanked {
                    state.add_overlay(Some(output), qh);
                }
            }
            // `global_remove` carries only the name; it may be any global.
            wl_registry::Event::GlobalRemove { name } => {
                let Some(i) = state.outputs.iter().position(|(n, _)| *n == name) else {
                    return;
                };
                let (_, output) = state.outputs.swap_remove(i);
                log(
                    INFO,
                    &format!("monitor disconnected ({} left)", state.outputs.len()),
                );
                // The compositor usually sends `closed` too; removing here
                // covers compositors that don't.
                state.remove_overlays(|o| o.output.as_ref() == Some(&output));
                if output.version() >= 3 {
                    output.release();
                }
            }
            _ => {}
        }
    }
}

// Output events (geometry, mode, name...) are not needed: layer-shell sizes
// the overlay for us.
delegate_noop!(State: ignore WlOutput);
