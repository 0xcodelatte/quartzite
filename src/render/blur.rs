//! Background blur.
//!
//! Like macOS's "vibrancy" materials, translucent surfaces show a blurred
//! version of what is behind them. To keep it cheap we blur only the
//! wallpaper layer (niri calls this "xray" blur): it is rendered once into
//! a quarter-size texture, and each blurred region samples that texture
//! through a Gaussian shader. Re-rendering is needed only when the output
//! size changes, so blur adds almost nothing per frame.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{texture::TextureRenderElement, Id, Kind},
            gles::{element::PixelShaderElement, GlesRenderer, GlesTexture},
            utils::draw_render_elements,
            Bind, Frame, Offscreen, Renderer,
        },
    },
    utils::{Logical, Physical, Rectangle, Size, Transform},
};

/// The wallpaper copy is this many times smaller than the output.
const DOWNSAMPLE: f64 = 4.0;

pub struct BlurCache {
    texture: GlesTexture,
    logical: Size<f64, Logical>,
    tex_size: Size<i32, Physical>,
}

impl BlurCache {
    pub fn new(
        renderer: &mut GlesRenderer,
        logical: Size<f64, Logical>,
        wallpaper: &PixelShaderElement,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let scale = 1.0 / DOWNSAMPLE;
        let tex_size: Size<i32, Physical> = logical.to_physical_precise_ceil(scale);
        let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (tex_size.w, tex_size.h).into())?;
        {
            let full = Rectangle::from_size(tex_size);
            let mut fb = renderer.bind(&mut texture)?;
            let mut frame = renderer.render(&mut fb, tex_size, Transform::Normal)?;
            frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[full])?;
            draw_render_elements(&mut frame, scale, std::slice::from_ref(wallpaper), &[full])?;
            let _ = frame.finish()?;
        }
        Ok(Self { texture, logical, tex_size })
    }

    pub fn matches(&self, logical: Size<f64, Logical>) -> bool {
        self.logical == logical
    }

    /// A texture element showing the part of the blurred wallpaper under
    /// `rect`. Returns it with the texel size the blur shader needs.
    pub fn element(
        &self,
        renderer: &mut GlesRenderer,
        id: &Id,
        rect: Rectangle<f64, Logical>,
        alpha: f32,
        scale: f64,
    ) -> (TextureRenderElement<GlesTexture>, [f32; 2]) {
        // `src` is in texture pixels (buffer scale 1).
        let src = Rectangle::new(rect.loc.downscale(DOWNSAMPLE), rect.size.downscale(DOWNSAMPLE));
        let elem = TextureRenderElement::from_static_texture(
            id.clone(),
            renderer.context_id(),
            rect.loc.to_physical(scale),
            self.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha),
            Some(src),
            Some(rect.size.to_i32_round()),
            None,
            Kind::Unspecified,
        );
        let texel = [1.0 / self.tex_size.w as f32, 1.0 / self.tex_size.h as f32];
        (elem, texel)
    }
}
