//! Workspaces ("Spaces") and window placement.
//!
//! Each workspace owns a Smithay `Space<Window>`, which stores window
//! positions and stacking order. All outputs are mapped into every space.

pub mod window;

use smithay::{
    desktop::{Space, Window},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle},
};

use crate::animation::Spring;

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

pub struct Layout {
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    outputs: Vec<Output>,
}

impl Layout {
    pub fn new() -> Self {
        Self {
            workspaces: vec![Workspace::new()],
            active: 0,
            outputs: Vec::new(),
        }
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
