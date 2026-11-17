//! Custom GLSL programs, compiled once per renderer.

use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};

pub struct Shaders {
    pub shadow: GlesPixelProgram,
    pub rounded: GlesTexProgram,
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
        Ok(Self { shadow, rounded })
    }
}
