//! Input handling: routes libinput/winit events to clients, implements
//! click-to-focus and the compositor's own key bindings.

pub mod grabs;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
        GestureSwipeUpdateEvent as _, InputBackend, InputEvent,
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
    Minimize,
    RestoreMinimized,
    /// Relative (+1/-1) space switch.
    SwitchSpaceBy(i32),
    SwitchSpace(usize),
    MoveToSpaceBy(i32),
    ToggleExpose,
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

fn binding(cmd: CmdKey, mods: &ModifiersState, sym: Keysym, expose: bool) -> Option<Action> {
    // Mission Control: Ctrl+↑ or F3; Escape leaves it.
    if (mods.ctrl && sym == Keysym::Up) || sym == Keysym::F3 || (expose && sym == Keysym::Escape) {
        return Some(Action::ToggleExpose);
    }
    if mods.ctrl && mods.alt && sym == Keysym::BackSpace {
        return Some(Action::Quit);
    }
    // xkb turns Ctrl+Alt+F1..F12 into XF86Switch_VT_1..12.
    let vt = sym.raw().wrapping_sub(Keysym::XF86_Switch_VT_1.raw());
    if vt < 12 {
        return Some(Action::ChangeVt(vt as i32 + 1));
    }
    // Spaces use Ctrl like macOS (Ctrl+←/→, Ctrl+1…4).
    if mods.ctrl && !mods.alt && !mods.logo {
        let dir = match sym {
            Keysym::Left => Some(-1),
            Keysym::Right => Some(1),
            _ => None,
        };
        if let Some(d) = dir {
            return Some(if mods.shift { Action::MoveToSpaceBy(d) } else { Action::SwitchSpaceBy(d) });
        }
        let n = sym.raw().wrapping_sub(Keysym::_1.raw());
        if n < crate::layout::SPACE_COUNT as u32 {
            return Some(Action::SwitchSpace(n as usize));
        }
    }
    if cmd.held(mods) {
        let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".into());
        return match sym {
            Keysym::q | Keysym::w => Some(Action::CloseWindow),
            Keysym::f => Some(Action::ToggleMaximize),
            Keysym::m => Some(Action::Minimize),
            Keysym::M => Some(Action::RestoreMinimized),
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
            // Three/four-finger horizontal swipes drag between Spaces.
            InputEvent::GestureSwipeBegin { event } => {
                self.swipe_fingers = event.fingers();
            }
            InputEvent::GestureSwipeUpdate { event } => {
                if self.swipe_fingers >= 3 {
                    let width = self.layout.usable_area().size.w.max(1) as f64;
                    self.layout.swipe_update(-event.delta_x() / width);
                    self.queue_redraw();
                }
            }
            InputEvent::GestureSwipeEnd { .. } => {
                if self.swipe_fingers >= 3 {
                    self.layout.swipe_end();
                    self.after_space_switch();
                }
                self.swipe_fingers = 0;
            }
            InputEvent::PointerAxis { event } => self.on_axis::<I>(event),
            _ => {}
        }
    }

    fn on_key<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let pressed = event.state() == KeyState::Pressed;
        let cmd = self.cmd_key;
        let expose = self.layout.expose_active();
        let action = self.seat.get_keyboard().unwrap().input(
            self,
            event.key_code(),
            event.state(),
            serial,
            time,
            |_, mods, handle| match pressed.then(|| binding(cmd, mods, handle.modified_sym(), expose)).flatten() {
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

        // In Exposé, a click picks a window instead of reaching clients.
        if self.layout.expose_active() {
            if state == ButtonState::Pressed {
                let pos = pointer.current_location();
                let picked = self
                    .layout
                    .expose_targets()
                    .into_iter()
                    .find(|(_, r)| r.contains(pos))
                    .map(|(w, _)| w);
                self.exit_expose(picked);
            }
            return;
        }

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
            Action::SwitchSpaceBy(d) => {
                self.layout.switch_by(d);
                self.after_space_switch();
            }
            Action::SwitchSpace(i) => {
                self.layout.switch_to(i);
                self.after_space_switch();
            }
            Action::MoveToSpaceBy(d) => {
                if let Some(window) = self.focused_window() {
                    let target = (self.layout.active as i32 + d).max(0) as usize;
                    self.layout.move_to_space(&window, target);
                    self.layout.switch_to(target);
                    self.after_space_switch();
                    self.focus_window(Some(window));
                }
            }
            Action::ToggleExpose => {
                if self.layout.expose_active() {
                    self.exit_expose(None);
                } else {
                    self.layout.set_expose(true);
                    // Clients shouldn't see hover/clicks while windows are thumbnails.
                    let pointer = self.seat.get_pointer().unwrap();
                    let loc = pointer.current_location();
                    pointer.motion(self, None, &MotionEvent { location: loc, serial: SERIAL_COUNTER.next_serial(), time: 0 });
                    pointer.frame(self);
                    self.queue_redraw();
                }
            }
            Action::Minimize => {
                if let Some(window) = self.focused_window() {
                    self.minimize_window(&window);
                }
            }
            Action::RestoreMinimized => {
                if let Some(window) = self.layout.active().minimized.last().cloned() {
                    self.restore_window(&window);
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

    /// Leave Exposé, optionally focusing (and un-minimizing) a window.
    pub fn exit_expose(&mut self, picked: Option<Window>) {
        self.layout.set_expose(false);
        if let Some(window) = picked {
            if self.layout.active().minimized.contains(&window) {
                self.restore_window(&window);
            } else {
                self.focus_window(Some(window));
            }
        }
        self.queue_redraw();
    }

    /// Focus follows the space: activate its topmost window.
    pub fn after_space_switch(&mut self) {
        let top = self.layout.active().space.elements().last().cloned();
        self.focus_window(top);
    }

    pub fn minimize_window(&mut self, window: &Window) {
        self.layout.minimize(window);
        let next = self.layout.active().space.elements().last().cloned();
        self.focus_window(next);
    }

    pub fn restore_window(&mut self, window: &Window) {
        self.layout.restore(window);
        self.focus_window(Some(window.clone()));
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
            let mut st = crate::layout::window::window_state(w);
            st.focused = Some(w) == window.as_ref();
            let target = if st.focused { 1.0 } else { 0.0 };
            st.focus.to(target, crate::animation::Spring::smooth());
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
