//! Pointer grabs for interactive move and resize.

use std::cell::RefCell;

use smithay::{
    desktop::Window,
    input::{
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
            RelativeMotionEvent,
        },
        Seat,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{protocol::wl_surface::WlSurface, Resource},
    },
    utils::{Logical, Point, Rectangle, Serial, Size},
    wayland::compositor::with_states,
};

use crate::{layout::Layout, state::State};

const BTN_LEFT: u32 = 0x110;

/// Validate that a client's move/resize request matches a real button press.
pub fn check_grab(seat: &Seat<State>, surface: &WlSurface, serial: Serial) -> Option<GrabStartData<State>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

/// The gesture callbacks are identical for every grab: pass them through.
macro_rules! forward_gestures {
    () => {
        fn relative_motion(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, focus: Option<(WlSurface, Point<f64, Logical>)>, event: &RelativeMotionEvent) { handle.relative_motion(data, focus, event) }
        fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) { handle.axis(data, details) }
        fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) { handle.frame(data) }
        fn gesture_swipe_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeBeginEvent) { handle.gesture_swipe_begin(data, event) }
        fn gesture_swipe_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeUpdateEvent) { handle.gesture_swipe_update(data, event) }
        fn gesture_swipe_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureSwipeEndEvent) { handle.gesture_swipe_end(data, event) }
        fn gesture_pinch_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchBeginEvent) { handle.gesture_pinch_begin(data, event) }
        fn gesture_pinch_update(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchUpdateEvent) { handle.gesture_pinch_update(data, event) }
        fn gesture_pinch_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GesturePinchEndEvent) { handle.gesture_pinch_end(data, event) }
        fn gesture_hold_begin(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldBeginEvent) { handle.gesture_hold_begin(data, event) }
        fn gesture_hold_end(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &GestureHoldEndEvent) { handle.gesture_hold_end(data, event) }
        fn start_data(&self) -> &GrabStartData<State> { &self.start_data }
        fn unset(&mut self, _data: &mut State) {}
    };
}

// ---------------------------------------------------------------- move

pub struct MoveSurfaceGrab {
    pub start_data: GrabStartData<State>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<State> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;
        let new_loc = (self.initial_window_location.to_f64() + delta).to_i32_round();
        data.layout.active_mut().space.map_element(self.window.clone(), new_loc, true);
        data.queue_redraw();
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_gestures!();
}

// ---------------------------------------------------------------- resize

const EDGE_TOP: u32 = 1;
const EDGE_LEFT: u32 = 4;

/// Per-surface resize bookkeeping, used to keep the opposite edge fixed
/// when resizing from the top or left.
#[derive(Default, Clone, Copy)]
struct ResizeData {
    edges: u32,
    initial: Option<Rectangle<i32, Logical>>,
    active: bool,
}

fn with_resize_data<T>(surface: &WlSurface, f: impl FnOnce(&mut ResizeData) -> T) -> T {
    with_states(surface, |states| {
        states.data_map.insert_if_missing(|| RefCell::new(ResizeData::default()));
        let mut data = states.data_map.get::<RefCell<ResizeData>>().unwrap().borrow_mut();
        f(&mut data)
    })
}

pub struct ResizeSurfaceGrab {
    start_data: GrabStartData<State>,
    window: Window,
    edges: u32,
    initial: Rectangle<i32, Logical>,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: GrabStartData<State>,
        window: Window,
        edges: xdg_toplevel::ResizeEdge,
        initial: Rectangle<i32, Logical>,
    ) -> Self {
        let edges = edges as u32;
        if let Some(toplevel) = window.toplevel() {
            with_resize_data(toplevel.wl_surface(), |d| {
                *d = ResizeData { edges, initial: Some(initial), active: true };
            });
        }
        Self { start_data, window, edges, initial }
    }
}

impl PointerGrab<State> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;
        let (mut w, mut h) = (self.initial.size.w as f64, self.initial.size.h as f64);
        if self.edges & EDGE_LEFT != 0 {
            w -= delta.x;
        } else if self.edges & 8 != 0 {
            w += delta.x;
        }
        if self.edges & EDGE_TOP != 0 {
            h -= delta.y;
        } else if self.edges & 2 != 0 {
            h += delta.y;
        }
        let size = Size::<i32, Logical>::from((w.max(120.0) as i32, h.max(80.0) as i32));
        if let Some(toplevel) = self.window.toplevel() {
            toplevel.with_pending_state(|s| s.size = Some(size));
            toplevel.send_pending_configure();
        }
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&BTN_LEFT) {
            if let Some(toplevel) = self.window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.states.unset(xdg_toplevel::State::Resizing);
                });
                toplevel.send_pending_configure();
                with_resize_data(toplevel.wl_surface(), |d| d.active = false);
            }
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_gestures!();
}

/// On commit, move the window so the edge opposite to the dragged one stays put.
pub fn handle_resize_commit(layout: &mut Layout, surface: &WlSurface) {
    let Some((ws, window)) = layout.find_window(surface) else { return };
    let data = with_resize_data(surface, |d| *d);
    let Some(initial) = data.initial else { return };
    if data.edges & (EDGE_TOP | EDGE_LEFT) == 0 {
        return;
    }
    let space = &mut layout.workspaces[ws].space;
    let Some(mut loc) = space.element_location(&window) else { return };
    let size = window.geometry().size;
    if data.edges & EDGE_LEFT != 0 {
        loc.x = initial.loc.x + (initial.size.w - size.w);
    }
    if data.edges & EDGE_TOP != 0 {
        loc.y = initial.loc.y + (initial.size.h - size.h);
    }
    space.map_element(window, loc, false);
    if !data.active {
        with_resize_data(surface, |d| d.initial = None);
    }
}
