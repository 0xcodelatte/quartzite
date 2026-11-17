//! Workspaces ("Spaces") and window placement.
//!
//! Each workspace owns a Smithay `Space<Window>`, which stores window
//! positions and stacking order. All outputs are mapped into every space.

use smithay::{
    desktop::{Space, Window},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
};

pub struct Workspace {
    pub space: Space<Window>,
}

impl Workspace {
    fn new() -> Self {
        Self { space: Space::default() }
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

    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.iter()
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

    pub fn remove_output(&mut self, output: &Output) {
        self.outputs.retain(|o| o != output);
        for ws in &mut self.workspaces {
            ws.space.unmap_output(output);
        }
    }

    /// Logical geometry of the primary (first) output.
    pub fn primary_output_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.outputs.first()?;
        self.active().space.output_geometry(output)
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

    /// Find which workspace a toplevel lives on.
    pub fn find_window(&self, surface: &WlSurface) -> Option<(usize, Window)> {
        self.workspaces.iter().enumerate().find_map(|(i, ws)| {
            ws.space
                .elements()
                .find(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false))
                .map(|w| (i, w.clone()))
        })
    }

    pub fn refresh(&mut self) {
        for ws in &mut self.workspaces {
            ws.space.refresh();
        }
    }
}
