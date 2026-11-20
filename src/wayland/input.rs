//! `ignore-mouse` feature: wake up on deliberate input only.
//!
//! The compositor's `resumed` fires on any input, mouse motion included, and
//! can't tell them apart. So with `--ignore-mouse` we ignore `resumed` and
//! unblank on input that reaches our overlays instead: they cover every screen
//! and grab the keyboard, so keys, buttons, scrolling and touch land on them.
//! Plain motion is never acted upon.
//!
//! We wake on *release* (key up, button up, touch up), so the whole press and
//! release pair is swallowed by the overlay and no application sees half of it.

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{
        wl_keyboard::{self, WlKeyboard},
        wl_pointer,
        wl_seat::{self, WlSeat},
        wl_touch::{self, WlTouch},
    },
};

use super::State;
use crate::logging::{INFO, log};

/// Input objects used only when `--ignore-mouse` is given.
pub struct IgnoreMouse {
    pub enabled: bool,
    keyboard: Option<WlKeyboard>,
    touch: Option<WlTouch>,
}

impl IgnoreMouse {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            keyboard: None,
            touch: None,
        }
    }
}

/// From the seat handler: request a keyboard and a touch object when the seat
/// has them. Skipped entirely without `--ignore-mouse`. (The pointer is always
/// requested by the base code, to hide the cursor.)
pub fn seat_capabilities(
    state: &mut State,
    seat: &WlSeat,
    caps: wl_seat::Capability,
    qh: &QueueHandle<State>,
) {
    let im = &mut state.ignore_mouse;
    if !im.enabled {
        return;
    }
    if caps.contains(wl_seat::Capability::Keyboard) && im.keyboard.is_none() {
        im.keyboard = Some(seat.get_keyboard(qh, ()));
    }
    if caps.contains(wl_seat::Capability::Touch) && im.touch.is_none() {
        im.touch = Some(seat.get_touch(qh, ()));
    }
}

/// From the pointer handler in wayland.rs, which owns the `WlPointer` Dispatch
/// impl (it also hides the cursor): a released button or scrolling wakes up.
/// Motion, enter and leave are ignored.
pub fn pointer_event(state: &mut State, event: wl_pointer::Event) {
    match event {
        wl_pointer::Event::Button {
            state: WEnum::Value(wl_pointer::ButtonState::Released),
            ..
        } => wake(state, "button"),
        wl_pointer::Event::Axis { .. } => wake(state, "scroll"),
        _ => {}
    }
}

/// Unblanks if `--ignore-mouse` is active and we are blanked. Events from the
/// overlays can only arrive while blanked, but a few may still be in flight
/// right after unblanking: the `blanked` check makes those harmless.
fn wake(state: &mut State, what: &str) {
    if state.ignore_mouse.enabled && state.blanked {
        log(INFO, &format!("{what}: screen restored"));
        state.hide();
    }
}

/// Keyboard focus comes from the overlay's exclusive keyboard interactivity.
/// The keymap event carries an fd, which is closed when the event is dropped.
impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            state: WEnum::Value(wl_keyboard::KeyState::Released),
            ..
        } = event
        {
            wake(state, "key");
        }
    }
}

/// A finger lifted from a touchscreen wakes up.
impl Dispatch<WlTouch, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_touch::Event::Up { .. } = event {
            wake(state, "touch");
        }
    }
}
