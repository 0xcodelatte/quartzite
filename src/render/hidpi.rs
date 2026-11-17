//! macOS-style fractional scaling.
//!
//! macOS never asks apps to draw at 1.5x. It tells them "2x", renders the
//! whole desktop into a 2x framebuffer and then scales that image down to
//! the panel's real resolution. We do the same: clients see an integer
//! scale of 2, the scene is composed at 2x into an offscreen texture, and
//! that texture is drawn onto the output with linear filtering.
//!
//! Damage still works end to end: the offscreen pass has its own damage
//! tracker, and the regions it repaints are forwarded to the final pass.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{texture::TextureRenderElement, Id, Kind},
            gles::{GlesRenderer, GlesTexture},
            utils::DamageBag,
            Bind, Offscreen, Renderer,
        },
    },
    output,
    utils::{Buffer, Logical, Physical, Rectangle, Size, Transform},
};

use super::{OutputElement, CLEAR_COLOR};

/// The integer scale the scene is composed at for fractional outputs.
pub const SUPERSAMPLE: i32 = 2;

/// Parse `QUARTZITE_SCALE` (e.g. `1`, `2`, `1.5`, `1.75`).
pub fn scale_from_env() -> output::Scale {
    let scale: f64 = std::env::var("QUARTZITE_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &f64| (0.5..=4.0).contains(s))
        .unwrap_or(1.0);
    if scale.fract() == 0.0 {
        output::Scale::Integer(scale as i32)
    } else {
        // Clients are told "2x"; we downscale to `scale` ourselves.
        output::Scale::Custom { advertised_integer: scale.ceil() as i32, fractional: scale }
    }
}

pub fn is_fractional(scale: output::Scale) -> bool {
    scale.fractional_scale().fract() != 0.0
}

pub struct Supersampler {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    tracker: OutputDamageTracker,
    damage: DamageBag<i32, Buffer>,
    id: Id,
}

impl Supersampler {
    pub fn new(renderer: &mut GlesRenderer, logical: Size<i32, Logical>) -> Result<Self, Box<dyn std::error::Error>> {
        let size = logical.to_physical(SUPERSAMPLE);
        let texture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
        Ok(Self {
            texture,
            size,
            tracker: OutputDamageTracker::new(size, SUPERSAMPLE as f64, Transform::Normal),
            damage: DamageBag::new(4),
            id: Id::new(),
        })
    }

    pub fn size(&self) -> Size<i32, Physical> {
        self.size
    }

    /// Compose `elements` (built at 2x) into the texture and return one
    /// element that draws it downscaled onto the real output.
    pub fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
    ) -> Result<TextureRenderElement<GlesTexture>, Box<dyn std::error::Error>> {
        {
            let mut fb = renderer.bind(&mut self.texture)?;
            // The texture keeps last frame's pixels, so its "age" is 1.
            let result = self.tracker.render_output(renderer, &mut fb, 1, elements, CLEAR_COLOR)?;
            if let Some(damage) = result.damage {
                self.damage.add(
                    damage
                        .iter()
                        .map(|r| Rectangle::<i32, Buffer>::new((r.loc.x, r.loc.y).into(), (r.size.w, r.size.h).into())),
                );
            }
        }
        let full = Rectangle::<i32, Buffer>::from_size((self.size.w, self.size.h).into());
        Ok(TextureRenderElement::from_texture_with_damage(
            self.id.clone(),
            renderer.context_id(),
            (0.0, 0.0),
            self.texture.clone(),
            SUPERSAMPLE,
            Transform::Normal,
            None,
            None,
            None,
            Some(vec![full]),
            self.damage.snapshot(),
            Kind::Unspecified,
        ))
    }
}
