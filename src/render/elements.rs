//! Custom render elements.

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Frame,
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
};

/// Wraps any texture-based element and clips it to a rounded rectangle by
/// swapping in our texture shader while the inner element draws.
pub struct RoundedElement<E> {
    inner: E,
    /// Window rectangle in output-local physical coordinates.
    geo: Rectangle<i32, Physical>,
    radius: f32,
    /// Untransformed output size, needed to map into framebuffer space.
    output_size: Size<i32, Physical>,
    program: GlesTexProgram,
}

impl<E> RoundedElement<E> {
    pub fn new(
        inner: E,
        geo: Rectangle<i32, Physical>,
        radius: f32,
        output_size: Size<i32, Physical>,
        program: GlesTexProgram,
    ) -> Self {
        Self { inner, geo, radius, output_size, program }
    }
}

impl<E: Element> Element for RoundedElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    /// The corners are now transparent, so they must not be reported as
    /// opaque — otherwise the damage tracker would skip drawing what is
    /// behind them. We keep only the "plus"-shaped part of each region.
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let r = self.radius.ceil() as i32;
        let g = self.geo;
        let loc = self.inner.geometry(scale).loc;
        // Opaque regions are relative to the element; express the cross there.
        let local = Rectangle::new(g.loc - loc, g.size);
        let horizontal = Rectangle::new(local.loc + Point::from((0, r)), Size::from((local.size.w, local.size.h - 2 * r)));
        let vertical = Rectangle::new(local.loc + Point::from((r, 0)), Size::from((local.size.w - 2 * r, local.size.h)));
        self.inner
            .opaque_regions(scale)
            .iter()
            .flat_map(|region| [region.intersection(horizontal), region.intersection(vertical)])
            .flatten()
            .collect()
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<E: RenderElement<GlesRenderer>> RenderElement<GlesRenderer> for RoundedElement<E> {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        // Smithay's projection maps transformed output coordinates straight
        // onto gl_FragCoord, so applying the output transform is enough.
        let fb = frame.transformation().transform_rect_in(self.geo, &self.output_size);
        let uniforms = vec![
            Uniform::new("geo", [fb.loc.x as f32, fb.loc.y as f32, fb.size.w as f32, fb.size.h as f32]),
            Uniform::new("radius", self.radius),
        ];
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let result = self.inner.draw(frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        result
    }

    /// Never hand the raw buffer to a hardware plane: it would skip clipping.
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
