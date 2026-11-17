//! Backends decide where composed frames are presented.
//!
//! - `winit`: a window inside an existing Wayland/X11 session (for testing)

pub mod winit;

use smithay::backend::renderer::gles::GlesRenderer;

pub enum Backend {
    /// Placeholder while the real backend is being initialised.
    None,
    Winit(winit::WinitData),
}

impl Backend {
    pub fn renderer(&mut self) -> Option<&mut GlesRenderer> {
        match self {
            Backend::None => None,
            Backend::Winit(data) => Some(data.backend.renderer()),
        }
    }

    /// Ask the backend to draw a new frame at the next vblank.
    pub fn schedule_render(&mut self) {
        match self {
            Backend::None => {}
            Backend::Winit(data) => data.backend.window().request_redraw(),
        }
    }
}
