//! Backends decide where composed frames are presented.
//!
//! - `winit`: a window inside an existing Wayland/X11 session (for testing)
//! - `udev`:  DRM/KMS on a TTY, with libseat sessions and libinput

#[cfg(feature = "udev")]
pub mod udev;
pub mod winit;

use smithay::backend::renderer::gles::GlesRenderer;

use crate::state::State;

pub enum Backend {
    /// Placeholder while the real backend is being initialised.
    None,
    Winit(winit::WinitData),
    #[cfg(feature = "udev")]
    Udev(udev::UdevData),
}

impl Backend {
    pub fn renderer(&mut self) -> Option<&mut GlesRenderer> {
        match self {
            Backend::None => None,
            Backend::Winit(data) => Some(data.backend.renderer()),
            #[cfg(feature = "udev")]
            Backend::Udev(data) => data.renderer(),
        }
    }
}

impl State {
    /// Draw a new frame if something changed. Winit paces us through its
    /// redraw requests; the DRM backend through vblank events.
    pub fn render_if_needed(&mut self) {
        match &mut self.backend {
            Backend::None => {}
            Backend::Winit(data) => data.backend.window().request_redraw(),
            #[cfg(feature = "udev")]
            Backend::Udev(_) => self.render_udev(),
        }
    }
}
