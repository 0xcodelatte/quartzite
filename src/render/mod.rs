//! Turns the scene into render elements, front (topmost) to back.
//!
//! Every client renders into its own buffer; we only ever sample those
//! buffers here, so the compositor is in full control of the final frame.
//! Animations are applied as transforms on top of the client buffers.

pub mod cursor;
pub mod elements;
pub mod hidpi;
pub mod shaders;

use std::{collections::HashMap, time::Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::MemoryRenderBufferRenderElement,
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                texture::TextureRenderElement,
                utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
                Id, Kind,
            },
            gles::{element::PixelShaderElement, GlesRenderer, GlesTexture, Uniform},
            utils::draw_render_elements,
            Bind, Frame, Offscreen, Renderer,
        },
    },
    desktop::{PopupManager, Window},
    output::Output,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::seat::WaylandFocus,
};

use crate::{
    animation::{lerp, lerp_rect, Animated, Spring},
    layout::{window::window_state, Layout},
};
use elements::RoundedElement;
use shaders::Shaders;

pub const CLEAR_COLOR: [f32; 4] = [0.11, 0.12, 0.15, 1.0];
pub const CORNER_RADIUS: f64 = 10.0;

type Transformed = RelocateRenderElement<RescaleRenderElement<CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>>;
pub type WindowElement = RoundedElement<Transformed>;

render_elements! {
    pub OutputElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Window=WindowElement,
    Shader=PixelShaderElement,
    Snapshot=RescaleRenderElement<TextureRenderElement<GlesTexture>>,
    Hidpi=TextureRenderElement<GlesTexture>,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// A window that has been destroyed but is still fading out, drawn from a
/// texture captured just before the client's buffers went away.
pub struct ClosingWindow {
    workspace: usize,
    texture: GlesTexture,
    /// Global logical rectangle the window occupied.
    rect: Rectangle<f64, Logical>,
    anim: Animated,
    id: Id,
}

/// GPU resources and caches that persist between frames.
pub struct RenderState {
    pub cursor: cursor::Cursor,
    shaders: Option<Shaders>,
    closing: Vec<ClosingWindow>,
    supersamplers: HashMap<String, hidpi::Supersampler>,
    /// Set while building a frame if any animation is still in flight;
    /// backends then schedule another frame for the next vblank.
    pub animating: bool,
}

impl RenderState {
    pub fn new() -> Self {
        Self { cursor: cursor::Cursor::load(), shaders: None, closing: Vec::new(), supersamplers: HashMap::new(), animating: false }
    }

    fn shaders(&mut self, renderer: &mut GlesRenderer) -> &Shaders {
        self.shaders
            .get_or_insert_with(|| Shaders::compile(renderer).expect("failed to compile shaders"))
    }
}

/// How a window should be drawn this frame.
pub struct WindowDraw {
    /// Where the window really is (output-local logical).
    pub base: Rectangle<f64, Logical>,
    /// Where it appears this frame, after animations.
    pub visual: Rectangle<f64, Logical>,
    pub alpha: f32,
    /// 0 = unfocused shadow, 1 = focused shadow.
    pub focus: f32,
}

impl WindowDraw {
    fn is_identity(&self) -> bool {
        self.base == self.visual && self.alpha >= 1.0
    }
}

/// Build the frame for `output`. `pointer` is the global cursor position
/// when the compositor draws its own cursor.
///
/// Integer scales are composed directly. Fractional scales are composed at
/// 2x offscreen and downscaled (see `hidpi`).
pub fn output_elements(
    renderer: &mut GlesRenderer,
    rs: &mut RenderState,
    layout: &Layout,
    output: &Output,
    pointer: Option<Point<f64, Logical>>,
) -> Vec<OutputElement> {
    let scale = output.current_scale();
    if !hidpi::is_fractional(scale) {
        let size = output.current_mode().map(|m| m.size).unwrap_or_default();
        let size = output.current_transform().transform_size(size);
        return scene_elements(renderer, rs, layout, output, pointer, scale.fractional_scale(), size);
    }

    let Some(logical) = layout.active().space.output_geometry(output).map(|g| g.size) else {
        return Vec::new();
    };
    let name = output.name();
    let needs_new = rs
        .supersamplers
        .get(&name)
        .is_none_or(|s| s.size() != logical.to_physical(hidpi::SUPERSAMPLE));
    if needs_new {
        match hidpi::Supersampler::new(renderer, logical) {
            Ok(s) => {
                rs.supersamplers.insert(name.clone(), s);
            }
            Err(err) => {
                tracing::error!(?err, "cannot create 2x buffer, falling back to direct scaling");
                let size = output.current_mode().map(|m| m.size).unwrap_or_default();
                return scene_elements(renderer, rs, layout, output, pointer, scale.fractional_scale(), size);
            }
        }
    }
    let size = rs.supersamplers[&name].size();
    let scene = scene_elements(renderer, rs, layout, output, pointer, hidpi::SUPERSAMPLE as f64, size);
    let sampler = rs.supersamplers.get_mut(&name).unwrap();
    match sampler.render(renderer, &scene) {
        Ok(elem) => vec![OutputElement::Hidpi(elem)],
        Err(err) => {
            tracing::error!(?err, "supersampled render failed");
            Vec::new()
        }
    }
}

/// Compose the scene at `scale` for a target of `output_size` physical
/// pixels (the output itself, or the 2x offscreen buffer).
fn scene_elements(
    renderer: &mut GlesRenderer,
    rs: &mut RenderState,
    layout: &Layout,
    output: &Output,
    pointer: Option<Point<f64, Logical>>,
    scale: f64,
    output_size: Size<i32, Physical>,
) -> Vec<OutputElement> {
    let now = Instant::now();
    let ws = layout.active();
    let space = &ws.space;
    let Some(output_geo) = space.output_geometry(output) else {
        return Vec::new();
    };
    let output_rect = Rectangle::from_size(output_geo.size.to_f64());
    rs.shaders(renderer);
    rs.animating = false;

    let mut elements = Vec::new();

    if let Some(pos) = pointer {
        if output_geo.to_f64().contains(pos) {
            elements.extend(rs.cursor.render_elements(renderer, pos - output_geo.loc.to_f64(), scale));
        }
    }

    // Closing windows fade and shrink from their snapshot.
    let mut closing = std::mem::take(&mut rs.closing);
    for c in closing.iter_mut().filter(|c| c.workspace == layout.active) {
        let p = c.anim.tick(now);
        rs.animating |= !c.anim.is_settled();
        let mut rect = c.rect;
        rect.loc -= output_geo.loc.to_f64();
        let loc: Point<f64, Physical> = rect.loc.to_physical(scale);
        let tex = TextureRenderElement::from_static_texture(
            c.id.clone(),
            renderer.context_id(),
            loc,
            c.texture.clone(),
            1,
            Transform::Normal,
            Some(p as f32),
            None,
            Some(rect.size.to_i32_round()),
            None,
            Kind::Unspecified,
        );
        let center = (rect.loc + rect.size.downscale(2.0).to_point()).to_physical_precise_round(scale);
        elements.push(OutputElement::Snapshot(RescaleRenderElement::from_element(tex, center, 0.9 + 0.1 * p)));
    }
    closing.retain(|c| !(c.anim.is_settled() && c.anim.value() <= 0.0));
    rs.closing = closing;

    // `elements()` is bottom-to-top; render elements are front-to-back.
    for window in space.elements().rev() {
        let Some(loc) = space.element_location(window) else { continue };
        let base = Rectangle::new((loc - output_geo.loc).to_f64(), window.geometry().size.to_f64());
        let draw = animate_window(rs, window, base, output_rect, now);
        push_window(renderer, rs, &mut elements, window, &draw, scale, output_size);
    }

    // Windows still shrinking toward the bottom of the screen.
    for window in ws.minimized.iter().rev() {
        let loc = window_state(window).restore_loc;
        let base = Rectangle::new((loc - output_geo.loc).to_f64(), window.geometry().size.to_f64());
        let draw = animate_window(rs, window, base, output_rect, now);
        if draw.alpha > 0.01 {
            push_window(renderer, rs, &mut elements, window, &draw, scale, output_size);
        }
    }
    elements
}

/// Sample this window's springs and compute where it appears this frame.
fn animate_window(
    rs: &mut RenderState,
    window: &Window,
    base: Rectangle<f64, Logical>,
    output_rect: Rectangle<f64, Logical>,
    now: Instant,
) -> WindowDraw {
    let mut st = window_state(window);
    let open = st.open.tick(now);
    let focus = st.focus.tick(now);
    let mini = st.minimize.tick(now);
    let mut animating = !(st.open.is_settled() && st.focus.is_settled() && st.minimize.is_settled());

    let mut visual = base;
    if let Some(frame) = &mut st.frame {
        visual = frame.tick(base, now);
        if frame.is_settled() {
            st.frame = None;
        } else {
            animating = true;
        }
    }

    // Open: grow from 90 % while fading in.
    visual = scale_about_center(visual, 0.9 + 0.1 * open);
    let mut alpha = open.clamp(0.0, 1.0);

    // Minimize: shrink toward the bottom centre of the screen ("dock").
    if mini > 0.0 {
        let dock = Rectangle::new(
            (output_rect.size.w / 2.0 - visual.size.w * 0.05, output_rect.size.h - visual.size.h * 0.1).into(),
            visual.size.downscale(10.0),
        );
        visual = lerp_rect(visual, dock, mini.clamp(0.0, 1.0));
        alpha *= lerp(1.0, 0.0, (mini * mini).clamp(0.0, 1.0));
    }

    rs.animating |= animating;
    WindowDraw { base, visual, alpha: alpha as f32, focus: focus as f32 }
}

fn scale_about_center(r: Rectangle<f64, Logical>, s: f64) -> Rectangle<f64, Logical> {
    let size = r.size.upscale(s);
    let loc = r.loc + (r.size.to_point() - size.to_point()).downscale(2.0);
    Rectangle::new(loc, size)
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
    if geo.size.is_empty() {
        return;
    }
    let base = draw.base;
    let surface_origin: Point<i32, Physical> = (base.loc - geo.loc.to_f64()).to_physical_precise_round(scale);
    let base_phys: Rectangle<i32, Physical> = base.to_physical_precise_round(scale);
    let visual_phys: Rectangle<i32, Physical> = draw.visual.to_physical_precise_round(scale);
    let program = rs.shaders.as_ref().unwrap().rounded.clone();

    // Popups only while the window is at rest; menus don't animate.
    if draw.is_identity() {
        for (popup, offset) in PopupManager::popups_for_surface(&surface) {
            let offset = (geo.loc + offset - popup.geometry().loc).to_physical_precise_round(scale);
            out.extend(
                render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    popup.wl_surface(),
                    surface_origin + offset,
                    Scale::from(scale),
                    1.0,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(OutputElement::Surface),
            );
        }
    }

    // Crop to the window geometry (drops client-side shadows), scale and
    // move to the animated rectangle, then clip the corners.
    let stretch = Scale::from((
        draw.visual.size.w / base.size.w,
        draw.visual.size.h / base.size.h,
    ));
    let offset = visual_phys.loc - base_phys.loc;
    let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        surface_origin,
        Scale::from(scale),
        draw.alpha,
        Kind::Unspecified,
    );
    let radius = (CORNER_RADIUS * scale * stretch.x.min(stretch.y)) as f32;
    for e in surfaces {
        let Some(cropped) = CropRenderElement::from_element(e, scale, base_phys) else { continue };
        let rescaled = RescaleRenderElement::from_element(cropped, base_phys.loc, stretch);
        let moved = RelocateRenderElement::from_element(rescaled, offset, Relocate::Relative);
        out.push(OutputElement::Window(RoundedElement::new(
            moved,
            visual_phys,
            radius,
            output_size,
            program.clone(),
        )));
    }

    out.push(OutputElement::Shader(shadow_element(rs, window, draw)));
}

/// The focused window gets a larger, darker shadow, like on macOS.
fn shadow_element(rs: &RenderState, window: &Window, draw: &WindowDraw) -> PixelShaderElement {
    let f = draw.focus as f64;
    let sigma = 10.0 + 14.0 * f;
    let offset_y = 5.0 + 9.0 * f;
    let strength = (0.22 + 0.23 * f) * (draw.alpha as f64).powi(2);
    let margin = (3.0 * 24.0 + 14.0f64).ceil();

    let rect = draw.visual;
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
    let radius = (CORNER_RADIUS * (draw.visual.size.w / draw.base.size.w).min(1.0)) as f32;
    let params = [area.size.w as f32, area.size.h as f32, inner[0], inner[1], inner[2], inner[3], sigma as f32 + radius, strength as f32 + offset_y as f32];
    let uniforms = || {
        vec![
            Uniform::new("area", [area.size.w as f32, area.size.h as f32]),
            Uniform::new("rect", inner),
            Uniform::new("radius", radius),
            Uniform::new("sigma", sigma as f32),
            Uniform::new("offset_y", offset_y as f32),
            Uniform::new("strength", strength as f32),
        ]
    };

    let mut state = window_state(window);
    let program = rs.shaders.as_ref().unwrap().shadow.clone();
    let elem = state
        .shadow
        .get_or_insert_with(|| PixelShaderElement::new(program, area, None, 1.0, uniforms(), Kind::Unspecified));
    elem.resize(area, None);
    if state.shadow_params != params {
        state.shadow.as_mut().unwrap().update_uniforms(uniforms());
        state.shadow_params = params;
    }
    state.shadow.clone().unwrap()
}

/// Capture a window (with rounded corners) into a texture so it can keep
/// animating after the client has destroyed it.
pub fn start_close_animation(
    renderer: &mut GlesRenderer,
    rs: &mut RenderState,
    window: &Window,
    workspace: usize,
    rect: Rectangle<f64, Logical>,
    scale: f64,
) {
    let Some(surface) = window.wl_surface() else { return };
    let geo = window.geometry();
    if geo.size.is_empty() {
        return;
    }
    let size: Size<i32, Physical> = geo.size.to_f64().to_physical_precise_ceil(scale);
    let full = Rectangle::from_size(size);
    let program = rs.shaders(renderer).rounded.clone();

    let origin: Point<i32, Physical> = (Point::<f64, Logical>::default() - geo.loc.to_f64()).to_physical_precise_round(scale);
    let elements: Vec<RoundedElement<CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>> =
        render_elements_from_surface_tree(renderer, &surface, origin, Scale::from(scale), 1.0, Kind::Unspecified)
            .into_iter()
            .filter_map(|e| CropRenderElement::from_element(e, scale, full))
            .map(|e| RoundedElement::new(e, full, (CORNER_RADIUS * scale) as f32, size, program.clone()))
            .collect();

    let result = (|| -> Result<GlesTexture, Box<dyn std::error::Error>> {
        let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
        {
            let mut fb = renderer.bind(&mut texture)?;
            let mut frame = renderer.render(&mut fb, size, Transform::Normal)?;
            frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[full])?;
            draw_render_elements(&mut frame, scale, &elements, &[full])?;
            let _ = frame.finish()?;
        }
        Ok(texture)
    })();

    match result {
        Ok(texture) => {
            let mut anim = Animated::new(1.0);
            anim.to(0.0, Spring::new(0.25, 1.0));
            rs.closing.push(ClosingWindow { workspace, texture, rect, anim, id: Id::new() });
        }
        Err(err) => tracing::warn!(?err, "could not snapshot closing window"),
    }
}
