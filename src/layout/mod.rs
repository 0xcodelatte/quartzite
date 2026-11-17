//! Workspaces ("Spaces") and window placement.
//!
//! Each workspace owns a Smithay `Space<Window>`, which stores window
//! positions and stacking order. All outputs are mapped into every space.

pub mod expose;
pub mod window;

use smithay::{
    desktop::{Space, Window},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle},
};

use std::{cell::RefCell, time::Instant};

use crate::animation::{Animated, Spring};

pub struct Workspace {
    pub space: Space<Window>,
    /// Minimized windows are unmapped from the space but kept alive.
    pub minimized: Vec<Window>,
}

impl Workspace {
    fn new() -> Self {
        Self { space: Space::default(), minimized: Vec::new() }
    }
}

/// Number of Spaces (virtual desktops).
pub const SPACE_COUNT: usize = 4;

pub struct Layout {
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    outputs: Vec<Output>,
    /// Horizontal camera position in "spaces": 1.0 = showing space 1.
    /// Springs toward `active`, which produces the slide transition.
    /// A Cell because the renderer advances it while only reading Layout.
    slide: RefCell<Animated>,
    /// True while a touchpad swipe is dragging the spaces directly.
    swiping: bool,
    /// Exposé progress: 0 = normal desktop, 1 = all windows in a grid.
    expose: RefCell<Animated>,
}

impl Layout {
    pub fn new() -> Self {
        Self {
            workspaces: (0..SPACE_COUNT).map(|_| Workspace::new()).collect(),
            active: 0,
            outputs: Vec::new(),
            slide: RefCell::new(Animated::new(0.0)),
            swiping: false,
            expose: RefCell::new(Animated::new(0.0)),
        }
    }

    // ------------------------------------------------------------ Exposé

    pub fn expose_active(&self) -> bool {
        self.expose.borrow().target() > 0.5
    }

    pub fn set_expose(&mut self, on: bool) {
        self.expose.borrow_mut().to(if on { 1.0 } else { 0.0 }, Spring::smooth());
    }

    /// Advance the Exposé spring; returns (progress, still moving).
    pub fn expose_progress(&self, now: Instant) -> (f64, bool) {
        let mut e = self.expose.borrow_mut();
        let v = e.tick(now);
        (v, !e.is_settled())
    }

    /// Exposé grid for the active space, in global coordinates.
    pub fn expose_targets(&self) -> Vec<(Window, Rectangle<f64, Logical>)> {
        expose::targets(self.active(), self.usable_area().to_f64())
    }

    // ------------------------------------------------------------ Spaces

    pub fn switch_to(&mut self, index: usize) {
        if index < self.workspaces.len() {
            self.active = index;
            self.slide.borrow_mut().to(index as f64, Spring::smooth());
        }
    }

    pub fn switch_by(&mut self, delta: i32) {
        let target = self.active as i32 + delta;
        if target >= 0 {
            self.switch_to(target as usize);
        }
    }

    /// Move `window` to another space, keeping its position.
    pub fn move_to_space(&mut self, window: &Window, index: usize) {
        if index >= self.workspaces.len() || index == self.active {
            return;
        }
        let Some(loc) = self.active().space.element_location(window) else { return };
        self.active_mut().space.unmap_elem(window);
        self.workspaces[index].space.map_element(window.clone(), loc, true);
    }

    /// Touchpad swipe: `delta` is in output widths (positive = go right).
    pub fn swipe_update(&mut self, delta: f64) {
        self.swiping = true;
        let mut slide = self.slide.borrow_mut();
        let max = (self.workspaces.len() - 1) as f64;
        // Rubber-band past the first/last space.
        let mut v = slide.value() + delta;
        if v < 0.0 || v > max {
            v = slide.value() + delta * 0.3;
        }
        slide.set(v);
    }

    /// Finish a swipe: settle on the nearest space.
    pub fn swipe_end(&mut self) {
        self.swiping = false;
        let v = self.slide.borrow().value();
        let target = v.round().clamp(0.0, (self.workspaces.len() - 1) as f64) as usize;
        self.switch_to(target);
    }

    /// Which spaces are on screen this frame and their horizontal offset
    /// in output widths. Also reports whether the slide is still moving.
    pub fn visible_spaces(&self, now: Instant) -> (Vec<(usize, f64)>, bool) {
        let mut slide = self.slide.borrow_mut();
        let pos = slide.tick(now);
        let moving = !slide.is_settled() || self.swiping;
        let visible = (0..self.workspaces.len())
            .map(|i| (i, i as f64 - pos))
            .filter(|(_, off)| off.abs() < 1.0)
            .collect();
        (visible, moving)
    }

    pub fn active(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    /// Outputs are laid out left to right in global coordinates.
    pub fn add_output(&mut self, output: &Output) {
        let x: i32 = self
            .outputs
            .iter()
            .filter_map(|o| self.active().space.output_geometry(o))
            .map(|g| g.size.w)
            .sum();
        self.outputs.push(output.clone());
        for ws in &mut self.workspaces {
            ws.space.map_output(output, (x, 0));
        }
    }

    /// Logical geometry of the primary (first) output.
    pub fn primary_output_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.outputs.first()?;
        self.active().space.output_geometry(output)
    }

    pub fn primary_scale(&self) -> f64 {
        self.outputs.first().map(|o| o.current_scale().fractional_scale()).unwrap_or(1.0)
    }

    /// Area windows may occupy on the primary output.
    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        self.primary_output_geometry()
            .unwrap_or_else(|| Rectangle::from_size((1280, 800).into()))
    }

    /// Centre a window in the usable area (done once it has a size).
    pub fn center_window(&mut self, window: &Window) {
        let area = self.usable_area();
        let size = window.geometry().size;
        let n = self.active().space.elements().count() as i32 - 1;
        let cascade = 22 * (n.max(0) % 6);
        let loc = area.loc
            + Point::from(((area.size.w - size.w).max(0) / 2 + cascade, (area.size.h - size.h).max(0) / 2 + cascade));
        if let Some((ws, _)) = window.toplevel().and_then(|t| self.find_window(t.wl_surface())) {
            self.workspaces[ws].space.map_element(window.clone(), loc, false);
        }
    }

    /// Place a new window: centered, cascading so windows don't stack exactly.
    pub fn map_new_window(&mut self, window: Window) {
        let area = self.usable_area();
        let n = self.active().space.elements().count() as i32;
        let cascade = Point::<i32, Logical>::from((24 * (n % 8), 24 * (n % 8)));
        // The real size is unknown until the first commit, so aim for the
        // centre of a typical 800x600 window.
        let loc = area.loc
            + Point::from(((area.size.w - 800).max(0) / 2, (area.size.h - 600).max(0) / 2))
            + cascade;
        self.active_mut().space.map_element(window, loc, true);
    }

    /// Find which workspace a toplevel lives on (minimized ones included).
    pub fn find_window(&self, surface: &WlSurface) -> Option<(usize, Window)> {
        let is = |w: &&Window| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false);
        self.workspaces.iter().enumerate().find_map(|(i, ws)| {
            ws.space
                .elements()
                .find(is)
                .or_else(|| ws.minimized.iter().find(is))
                .map(|w| (i, w.clone()))
        })
    }

    /// Start the minimize animation and take the window out of the space.
    pub fn minimize(&mut self, window: &Window) {
        let ws = self.active_mut();
        let Some(loc) = ws.space.element_location(window) else { return };
        {
            let mut st = window::window_state(window);
            st.restore_loc = loc;
            st.minimize.to(1.0, Spring::smooth());
        }
        ws.space.unmap_elem(window);
        ws.minimized.push(window.clone());
    }

    /// Bring a minimized window back to where it was.
    pub fn restore(&mut self, window: &Window) {
        let ws = self.active_mut();
        ws.minimized.retain(|w| w != window);
        let loc = {
            let mut st = window::window_state(window);
            st.minimize.to(0.0, Spring::smooth());
            st.restore_loc
        };
        ws.space.map_element(window.clone(), loc, true);
    }

    pub fn refresh(&mut self) {
        for ws in &mut self.workspaces {
            ws.space.refresh();
            ws.minimized.retain(|w| w.alive());
        }
    }
}
