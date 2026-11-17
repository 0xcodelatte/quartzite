//! Custom GLSL programs, compiled once per renderer.

use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};

pub struct Shaders {
    pub shadow: GlesPixelProgram,
    pub rounded: GlesTexProgram,
    pub blur: GlesTexProgram,
    pub wallpaper: GlesPixelProgram,
}

impl Shaders {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        let shadow = renderer.compile_custom_pixel_shader(
            include_str!("shadow.frag"),
            &[
                UniformName::new("area", UniformType::_2f),
                UniformName::new("rect", UniformType::_4f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("sigma", UniformType::_1f),
                UniformName::new("offset_y", UniformType::_1f),
                UniformName::new("strength", UniformType::_1f),
            ],
        )?;
        let rounded = renderer.compile_custom_texture_shader(
            include_str!("rounded.frag"),
            &[
                UniformName::new("geo", UniformType::_4f),
                UniformName::new("radius", UniformType::_1f),
            ],
        )?;
        let blur = renderer.compile_custom_texture_shader(
            include_str!("blur.frag"),
            &[
                UniformName::new("geo", UniformType::_4f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("texel", UniformType::_2f),
                UniformName::new("tint_strength", UniformType::_1f),
            ],
        )?;
        let wallpaper = renderer.compile_custom_pixel_shader(include_str!("wallpaper.frag"), &[])?;
        Ok(Self { shadow, rounded, blur, wallpaper })
    }
}
