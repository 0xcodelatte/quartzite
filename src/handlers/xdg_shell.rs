//! xdg-shell: application windows (toplevels) and popups.

use smithay::{
    delegate_xdg_shell,
    desktop::{find_popup_root_surface, get_popup_toplevel_coords, PopupKind, Window},
    input::{pointer::Focus, Seat},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_seat, wl_surface::WlSurface},
    },
    utils::{Rectangle, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{
    animation::{AnimatedRect, Spring},
    input::grabs::{check_grab, MoveSurfaceGrab, ResizeSurfaceGrab},
    layout::window::window_state,
    render,
    state::State,
};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.layout.map_new_window(window.clone());
        self.focus_window(Some(window));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // Snapshot the window now: its buffers are about to disappear.
        if let Some((ws, window)) = self.layout.find_window(surface.wl_surface()) {
            let space = &self.layout.workspaces[ws].space;
            if let Some(loc) = space.element_location(&window) {
                let rect = Rectangle::new(loc.to_f64(), window.geometry().size.to_f64());
                let scale = self.layout.primary_scale();
                if let Some(renderer) = self.backend.renderer() {
                    render::start_close_animation(renderer, &mut self.render_state, &window, ws, rect, scale);
                }
            }
            self.layout.workspaces[ws].space.unmap_elem(&window);
        }
        // Give focus to the next window down the stack.
        self.layout.refresh();
        let next = self.layout.active().space.elements().last().cloned();
        self.focus_window(next);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else { return };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else { return };
        let Some((_, window)) = self.layout.find_window(surface.wl_surface()) else { return };
        let Some(initial_window_location) = self.layout.active().space.element_location(&window) else {
            return;
        };
        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
        };
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else { return };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else { return };
        let Some((_, window)) = self.layout.find_window(surface.wl_surface()) else { return };
        let Some(loc) = self.layout.active().space.element_location(&window) else { return };
        let size = window.geometry().size;

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();

        let grab = ResizeSurfaceGrab::start(start_data, window, edges, Rectangle::new(loc, size));
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some((_, window)) = self.layout.find_window(surface.wl_surface()) {
            self.minimize_window(&window);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(&surface, true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(&surface, false);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Popup grabs (menus that close on outside click) are not implemented yet.
    }
}
delegate_xdg_shell!(State);

impl State {
    /// Send the initial configure once the client has committed its role.
    pub fn handle_xdg_commit(&mut self, surface: &WlSurface) {
        if let Some((_, window)) = self.layout.find_window(surface) {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|d| d.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });
            if !initial_configure_sent {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_configure();
                }
            }
            // The first commit with content starts the open animation.
            let mut st = window_state(&window);
            if !st.open_started && !window.geometry().size.is_empty() {
                st.open_started = true;
                st.open.set(0.0);
                st.open.to(1.0, Spring::snappy());
            }
        }

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                // The initial configure is always allowed, so this cannot fail.
                let _ = xdg.send_configure();
            }
        }
    }

    /// Toggle the "zoom" state (macOS green button).
    pub fn set_maximized(&mut self, surface: &ToplevelSurface, maximize: bool) {
        let Some((_, window)) = self.layout.find_window(surface.wl_surface()) else { return };
        let area = self.layout.usable_area();
        // Animate the frame from where it is now to wherever it ends up.
        if let Some(loc) = self.layout.active().space.element_location(&window) {
            let from = Rectangle::new(loc.to_f64(), window.geometry().size.to_f64());
            window_state(&window).frame = Some(AnimatedRect::new(from));
        }
        if maximize {
            let loc = self.layout.active().space.element_location(&window).unwrap_or_default();
            window_state(&window).restore_loc = loc;
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(area.size);
            });
            self.layout.active_mut().space.map_element(window, area.loc, true);
        } else {
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            });
            let loc = window_state(&window).restore_loc;
            self.layout.active_mut().space.map_element(window, loc, true);
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else { return };
        let Some((_, window)) = self.layout.find_window(&root) else { return };
        let space = &self.layout.active().space;
        let Some(window_geo) = space.element_geometry(&window) else { return };

        // The positioner wants the target area relative to the parent window.
        let mut target = self.layout.usable_area();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
