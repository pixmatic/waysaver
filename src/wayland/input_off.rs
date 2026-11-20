//! Stub of `input.rs`, compiled when the `ignore-mouse` feature is off: the
//! same API, doing nothing. See `input.rs` for what each item means.
//!
//! Everything here is a constant or a no-op, so the compiler removes it.
#![allow(dead_code)]

use wayland_client::{
    QueueHandle,
    protocol::{wl_pointer, wl_seat},
};

use super::State;

pub const USAGE: &[&str] = &[];

#[derive(Default)]
pub struct Options {}

impl Options {
    pub fn option(
        &mut self,
        _arg: &str,
        _args: &mut dyn Iterator<Item = String>,
    ) -> Result<bool, String> {
        Ok(false)
    }
}

pub struct IgnoreMouse {}

impl IgnoreMouse {
    pub fn new(_options: &Options) -> Self {
        Self {}
    }
}

pub fn defers_resume(_state: &State) -> bool {
    false
}

pub fn seat_capabilities(
    _state: &mut State,
    _seat: &wl_seat::WlSeat,
    _caps: wl_seat::Capability,
    _qh: &QueueHandle<State>,
) {
}

pub fn pointer_event(_state: &mut State, _event: wl_pointer::Event) {}

#[cfg(test)]
mod tests {
    use crate::cli::test_util::run_blank;

    #[test]
    fn ignore_mouse_unknown_without_feature() {
        assert!(run_blank(&["--ignore-mouse", "10"]).is_err());
    }
}
