//! Stub of `outputs.rs`, compiled when the `multimonitor` feature is off: a
//! single target, `None` ("whichever output the compositor picks"), no names
//! and no hotplug. See `outputs.rs` for what each item means.
#![allow(dead_code)]

use wayland_client::{
    QueueHandle,
    globals::GlobalList,
    protocol::{
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
    },
};

use super::State;

pub struct Outputs {}

impl Outputs {
    pub fn bind_all(_globals: &GlobalList, _qh: &QueueHandle<State>) -> Self {
        Self {}
    }

    pub fn targets(&self) -> Vec<Option<WlOutput>> {
        vec![None]
    }

    pub fn name(&self, _target: Option<&WlOutput>) -> Option<String> {
        None
    }
}

/// Globals added or removed later are ignored: everything was bound once at
/// startup.
pub fn registry_event(
    _state: &mut State,
    _registry: &WlRegistry,
    _event: wl_registry::Event,
    _qh: &QueueHandle<State>,
) {
}
