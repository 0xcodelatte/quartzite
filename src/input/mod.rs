//! Input handling: routes libinput/winit events to clients, implements
//! click-to-focus and the compositor's own key bindings.

pub mod grabs;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::Window,
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::state::State;

/// Things the compositor does in response to a key binding.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    CloseWindow,
    ToggleMaximize,
    Spawn(String),
    /// Ctrl+Alt+Fn on a TTY.
    ChangeVt(i32),
}

/// Which physical modifier plays the role of macOS's ⌘ key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmdKey {
    Super,
    Alt,
}

impl CmdKey {
    /// `QUARTZITE_CMD=alt|super`; defaults to Alt when nested (the host
    /// desktop usually grabs Super) and Super on a TTY.
    pub fn from_env(nested: bool) -> Self {
        match std::env::var("QUARTZITE_CMD").as_deref() {
            Ok("alt") => CmdKey::Alt,
            Ok("super") => CmdKey::Super,
            _ if nested => CmdKey::Alt,
            _ => CmdKey::Super,
        }
    }

    fn held(self, mods: &ModifiersState) -> bool {
        match self {
            CmdKey::Super => mods.logo,
            CmdKey::Alt => mods.alt,
        }
    }
}

fn binding(cmd: CmdKey, mods: &ModifiersState, sym: Keysym) -> Option<Action> {
    if mods.ctrl && mods.alt && sym == Keysym::BackSpace {
        return Some(Action::Quit);
    }
    // xkb turns Ctrl+Alt+F1..F12 into XF86Switch_VT_1..12.
    let vt = sym.raw().wrapping_sub(Keysym::XF86_Switch_VT_1.raw());
    if vt < 12 {
        return Some(Action::ChangeVt(vt as i32 + 1));
    }
    if cmd.held(mods) {
        let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".into());
        return match sym {
            Keysym::q | Keysym::w => Some(Action::CloseWindow),
            Keysym::f => Some(Action::ToggleMaximize),
            Keysym::Return => Some(Action::Spawn(terminal)),
            _ => None,
        };
    }
    None
}

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => self.on_key::<I>(event),
            InputEvent::PointerMotion { event } => {
                let pointer = self.seat.get_pointer().unwrap();
                let pos = self.clamp_to_outputs(pointer.current_location() + event.delta());
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under.clone(),
                    &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time: event.time_msec() },
                );
                pointer.relative_motion(
                    self,
                    under,
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        utime: event.time(),
                    },
                );
                pointer.frame(self);
                self.queue_redraw();
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(geo) = self.layout.primary_output_geometry() else { return };
                let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time: event.time_msec() },
                );
                pointer.frame(self);
                self.queue_redraw();
            }
            InputEvent::PointerButton { event } => self.on_button::<I>(event),
            InputEvent::PointerAxis { event } => self.on_axis::<I>(event),
            _ => {}
        }
    }

    fn on_key<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let pressed = event.state() == KeyState::Pressed;
        let cmd = self.cmd_key;
        let action = self.seat.get_keyboard().unwrap().input(
            self,
            event.key_code(),
            event.state(),
            serial,
            time,
            |_, mods, handle| match pressed.then(|| binding(cmd, mods, handle.modified_sym())).flatten() {
                Some(action) => FilterResult::Intercept(action),
                None => FilterResult::Forward,
            },
        );
        if let Some(action) = action {
            self.run_action(action);
        }
    }

    fn on_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        let pointer = self.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        let state = event.state();

        // Click to focus: the clicked window is raised and activated.
        if state == ButtonState::Pressed && !pointer.is_grabbed() {
            let window = self.window_under(pointer.current_location());
            self.focus_window(window);
        }

        pointer.button(
            self,
            &ButtonEvent { button: event.button_code(), state, serial, time: event.time_msec() },
        );
        pointer.frame(self);
    }

    fn on_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();
        let mut frame = AxisFrame::new(event.time_msec()).source(source);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let amount = event
                .amount(axis)
                .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0);
            if amount != 0.0 {
                frame = frame.value(axis, amount);
                if let Some(v120) = event.amount_v120(axis) {
                    frame = frame.v120(axis, v120 as i32);
                }
            }
            if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                frame = frame.stop(axis);
            }
        }
        let pointer = self.seat.get_pointer().unwrap();
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.loop_signal.stop(),
            Action::Spawn(cmd) => self.spawn(&cmd),
            #[cfg(feature = "udev")]
            Action::ChangeVt(vt) => self.change_vt(vt),
            #[cfg(not(feature = "udev"))]
            Action::ChangeVt(_) => {}
            Action::CloseWindow => {
                if let Some(toplevel) = self.focused_window().and_then(|w| w.toplevel().cloned()) {
                    toplevel.send_close();
                }
            }
            Action::ToggleMaximize => {
                if let Some(toplevel) = self.focused_window().and_then(|w| w.toplevel().cloned()) {
                    let maximized = toplevel.with_pending_state(|s| {
                        s.states
                            .contains(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized)
                    });
                    self.set_maximized(&toplevel, !maximized);
                }
            }
        }
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.layout.find_window(&focus).map(|(_, w)| w)
    }

    /// Raise and activate `window` (or clear focus when `None`).
    pub fn focus_window(&mut self, window: Option<Window>) {
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        let space = &mut self.layout.active_mut().space;
        if let Some(window) = &window {
            space.raise_element(window, true);
        } else {
            for w in space.elements() {
                w.set_activated(false);
            }
        }
        for w in space.elements() {
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        let surface: Option<WlSurface> = window.and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
        keyboard.set_focus(self, surface, serial);
        self.queue_redraw();
    }

    fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let Some(geo) = self.layout.primary_output_geometry() else { return pos };
        let max_x = (geo.loc.x + geo.size.w) as f64 - 1.0;
        let max_y = (geo.loc.y + geo.size.h) as f64 - 1.0;
        (pos.x.clamp(geo.loc.x as f64, max_x), pos.y.clamp(geo.loc.y as f64, max_y)).into()
    }
}
