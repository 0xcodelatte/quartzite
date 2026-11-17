//! Per-window compositor state, stored in the window's user data.

use std::cell::{RefCell, RefMut};

use smithay::{backend::renderer::gles::element::PixelShaderElement, desktop::Window};

#[derive(Default)]
pub struct WindowState {
    pub focused: bool,
    /// Cached shadow element. Reusing it keeps its id stable, which lets
    /// the damage tracker skip it when nothing changed.
    pub shadow: Option<PixelShaderElement>,
    pub shadow_params: [f32; 8],
}

pub fn window_state(window: &Window) -> RefMut<'_, WindowState> {
    let data = window.user_data();
    data.insert_if_missing(|| RefCell::new(WindowState::default()));
    data.get::<RefCell<WindowState>>().unwrap().borrow_mut()
}
