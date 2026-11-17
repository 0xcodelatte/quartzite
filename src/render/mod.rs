//! Turns the scene into render elements, front (topmost) to back.
//!
//! Every client renders into its own buffer; we only ever sample those
//! buffers here, so the compositor is in full control of the final frame.

pub mod cursor;

use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, surface::WaylandSurfaceRenderElement, AsRenderElements},
        gles::GlesRenderer,
    },
    output::Output,
    render_elements,
    utils::{Logical, Point, Scale},
};

use crate::layout::Layout;

/// GPU resources and caches that persist between frames.
pub struct RenderState {
    pub cursor: cursor::Cursor,
}

impl RenderState {
    pub fn new() -> Self {
        Self { cursor: cursor::Cursor::load() }
    }
}

pub const CLEAR_COLOR: [f32; 4] = [0.11, 0.12, 0.15, 1.0];

render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Build the frame for `output`. `pointer` is the global cursor position
/// when the compositor draws its own cursor.
pub fn output_elements(
    renderer: &mut GlesRenderer,
    rs: &mut RenderState,
    layout: &Layout,
    output: &Output,
    pointer: Option<Point<f64, Logical>>,
) -> Vec<OutputElement> {
    let space = &layout.active().space;
    let Some(output_geo) = space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();

    let mut elements = Vec::new();

    if let Some(pos) = pointer {
        if output_geo.to_f64().contains(pos) {
            elements.extend(rs.cursor.render_elements(renderer, pos - output_geo.loc.to_f64(), scale));
        }
    }

    // `elements()` is bottom-to-top; render elements are front-to-back.
    for window in space.elements().rev() {
        let Some(loc) = space.element_location(window) else { continue };
        let render_loc = (loc - window.geometry().loc - output_geo.loc).to_physical_precise_round(scale);
        elements.extend(
            window
                .render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    render_loc,
                    Scale::from(scale),
                    1.0,
                )
                .into_iter()
                .map(OutputElement::Surface),
        );
    }
    elements
}
