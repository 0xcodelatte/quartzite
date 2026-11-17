//! Turns the scene into render elements, front (topmost) to back.
//!
//! Every client renders into its own buffer; we only ever sample those
//! buffers here, so the compositor is in full control of the final frame.

use smithay::{
    backend::renderer::{
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
        gles::GlesRenderer,
    },
    output::Output,
    render_elements,
    utils::Scale,
};

use crate::layout::Layout;

pub const CLEAR_COLOR: [f32; 4] = [0.11, 0.12, 0.15, 1.0];

render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
}

pub fn output_elements(
    renderer: &mut GlesRenderer,
    layout: &Layout,
    output: &Output,
) -> Vec<OutputElement> {
    let space = &layout.active().space;
    let Some(output_geo) = space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();

    let mut elements = Vec::new();
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
