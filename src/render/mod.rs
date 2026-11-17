//! Turns the scene into render elements, front (topmost) to back.
//!
//! Every client renders into its own buffer; we only ever sample those
//! buffers here, so the compositor is in full control of the final frame.

pub mod cursor;
pub mod elements;
pub mod shaders;

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            utils::CropRenderElement,
            Kind,
        },
        gles::{element::PixelShaderElement, GlesRenderer, Uniform},
    },
    desktop::{PopupManager, Window},
    output::Output,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::seat::WaylandFocus,
};

use crate::layout::{window::window_state, Layout};
use elements::RoundedElement;
use shaders::Shaders;

pub const CLEAR_COLOR: [f32; 4] = [0.11, 0.12, 0.15, 1.0];
pub const CORNER_RADIUS: f64 = 10.0;

pub type WindowElement = RoundedElement<CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>;

render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Window=WindowElement,
    Shader=PixelShaderElement,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// GPU resources and caches that persist between frames.
pub struct RenderState {
    pub cursor: cursor::Cursor,
    shaders: Option<Shaders>,
}

impl RenderState {
    pub fn new() -> Self {
        Self { cursor: cursor::Cursor::load(), shaders: None }
    }

    fn shaders(&mut self, renderer: &mut GlesRenderer) -> &Shaders {
        self.shaders
            .get_or_insert_with(|| Shaders::compile(renderer).expect("failed to compile shaders"))
    }
}

/// How a window should be drawn this frame.
pub struct WindowDraw {
    /// Window geometry in output-local logical coordinates.
    pub rect: Rectangle<f64, Logical>,
    pub alpha: f32,
    /// 0 = unfocused shadow, 1 = focused shadow.
    pub focus: f32,
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
    let output_size = output.current_mode().map(|m| m.size).unwrap_or_default();
    rs.shaders(renderer);

    let mut elements = Vec::new();

    if let Some(pos) = pointer {
        if output_geo.to_f64().contains(pos) {
            elements.extend(rs.cursor.render_elements(renderer, pos - output_geo.loc.to_f64(), scale));
        }
    }

    // `elements()` is bottom-to-top; render elements are front-to-back.
    for window in space.elements().rev() {
        let Some(loc) = space.element_location(window) else { continue };
        let focused = window_state(window).focused;
        let draw = WindowDraw {
            rect: Rectangle::new((loc - output_geo.loc).to_f64(), window.geometry().size.to_f64()),
            alpha: 1.0,
            focus: if focused { 1.0 } else { 0.0 },
        };
        push_window(renderer, rs, &mut elements, window, &draw, scale, output_size);
    }
    elements
}

/// Popups, then the rounded window, then its shadow (front to back).
fn push_window(
    renderer: &mut GlesRenderer,
    rs: &mut RenderState,
    out: &mut Vec<OutputElement>,
    window: &Window,
    draw: &WindowDraw,
    scale: f64,
    output_size: Size<i32, Physical>,
) {
    let Some(surface) = window.wl_surface() else { return };
    let geo = window.geometry();
    let rect = draw.rect;
    let surface_origin: Point<i32, Physical> = (rect.loc - geo.loc.to_f64()).to_physical_precise_round(scale);
    let geo_phys: Rectangle<i32, Physical> = rect.to_physical_precise_round(scale);
    let program = rs.shaders.as_ref().unwrap().rounded.clone();

    for (popup, offset) in PopupManager::popups_for_surface(&surface) {
        let offset = (geo.loc + offset - popup.geometry().loc).to_physical_precise_round(scale);
        out.extend(
            render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                popup.wl_surface(),
                surface_origin + offset,
                Scale::from(scale),
                draw.alpha,
                Kind::Unspecified,
            )
            .into_iter()
            .map(OutputElement::Surface),
        );
    }

    // Cropping to the window geometry removes client-side shadows, so ours
    // is the only one. Rounding then clips the corners.
    let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        surface_origin,
        Scale::from(scale),
        draw.alpha,
        Kind::Unspecified,
    );
    for e in surfaces {
        if let Some(cropped) = CropRenderElement::from_element(e, scale, geo_phys) {
            out.push(OutputElement::Window(RoundedElement::new(
                cropped,
                geo_phys,
                (CORNER_RADIUS * scale) as f32,
                output_size,
                program.clone(),
            )));
        }
    }

    out.push(OutputElement::Shader(shadow_element(rs, window, draw)));
}

/// The focused window gets a larger, darker shadow, like on macOS.
fn shadow_element(rs: &RenderState, window: &Window, draw: &WindowDraw) -> PixelShaderElement {
    let f = draw.focus as f64;
    let sigma = 10.0 + 14.0 * f;
    let offset_y = 5.0 + 9.0 * f;
    let strength = (0.22 + 0.23 * f) * draw.alpha as f64;
    let margin = (3.0 * 24.0 + 14.0f64).ceil();

    let rect = draw.rect;
    let area: Rectangle<i32, Logical> = Rectangle::new(
        (rect.loc.x - margin, rect.loc.y - margin).into(),
        (rect.size.w + 2.0 * margin, rect.size.h + 2.0 * margin).into(),
    )
    .to_i32_round();
    let inner = [
        (rect.loc.x - area.loc.x as f64) as f32,
        (rect.loc.y - area.loc.y as f64) as f32,
        rect.size.w as f32,
        rect.size.h as f32,
    ];
    let params = [
        area.size.w as f32, area.size.h as f32, inner[2], inner[3],
        sigma as f32, offset_y as f32, strength as f32, inner[0] + inner[1],
    ];
    let uniforms = || {
        vec![
            Uniform::new("area", [area.size.w as f32, area.size.h as f32]),
            Uniform::new("rect", inner),
            Uniform::new("radius", CORNER_RADIUS as f32),
            Uniform::new("sigma", sigma as f32),
            Uniform::new("offset_y", offset_y as f32),
            Uniform::new("strength", strength as f32),
        ]
    };

    let mut state = window_state(window);
    let program = rs.shaders.as_ref().unwrap().shadow.clone();
    let elem = state.shadow.get_or_insert_with(|| {
        PixelShaderElement::new(program, area, None, 1.0, uniforms(), Kind::Unspecified)
    });
    elem.resize(area, None);
    if state.shadow_params != params {
        state.shadow.as_mut().unwrap().update_uniforms(uniforms());
        state.shadow_params = params;
    }
    state.shadow.clone().unwrap()
}
