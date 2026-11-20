//! Stub of `theme.rs`, compiled when the `command` feature is off: overlays
//! are always black and there are no children. See `theme.rs` for what each
//! item means.
//!
//! Everything here is a no-op, so the compiler removes it.
#![allow(dead_code)]

use std::os::fd::BorrowedFd;

use wayland_client::{
    QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm::WlShm},
};

use super::{Overlay, State};
use crate::cli::Config;

pub struct Theme {}

impl Theme {
    pub fn new(_config: &Config, _shm: &WlShm, _qh: &QueueHandle<State>) -> Result<Self, String> {
        Ok(Self {})
    }
}

#[derive(Default)]
pub struct OverlayExt {}

pub fn configured(_overlay: &mut Overlay) {}

pub fn buffer_for<'a>(_state: &'a State, _overlay: &Overlay) -> Option<&'a WlBuffer> {
    None
}

pub fn add_backdrop(_state: &mut State, _output: Option<WlOutput>, _qh: &QueueHandle<State>) {}

pub fn start_all(_state: &mut State) {}

pub fn ensure_started(_state: &mut State, _target: WlOutput) {}

pub fn stop_all(_state: &mut State) {}

pub fn stop(_state: &mut State, _output: &WlOutput) {}

pub fn fds(_state: &State) -> Vec<BorrowedFd<'_>> {
    Vec::new()
}

pub fn reap(_state: &mut State) {}
