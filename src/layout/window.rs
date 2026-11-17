//! Per-window compositor state, stored in the window's user data.

use std::cell::{RefCell, RefMut};

use smithay::{
    backend::renderer::gles::element::PixelShaderElement,
    desktop::Window,
    utils::{Logical, Point},
};

use crate::animation::{Animated, AnimatedRect};

pub struct WindowState {
    pub focused: bool,
    /// 0 → 1 when the window first appears.
    pub open: Animated,
    pub open_started: bool,
    /// Shadow strength: 0 unfocused, 1 focused.
    pub focus: Animated,
    /// 0 normal, 1 minimized.
    pub minimize: Animated,
    /// Where to put the window back when it is restored.
    pub restore_loc: Point<i32, Logical>,
    /// Active while the frame animates to a new size (zoom/unzoom).
    pub frame: Option<AnimatedRect>,

    /// Cached shadow element. Reusing it keeps its id stable, which lets
    /// the damage tracker skip it when nothing changed.
    pub shadow: Option<PixelShaderElement>,
    pub shadow_params: [f32; 8],
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            focused: false,
            open: Animated::new(1.0),
            open_started: false,
            focus: Animated::new(0.0),
            minimize: Animated::new(0.0),
            restore_loc: Point::default(),
            frame: None,
            shadow: None,
            shadow_params: [0.0; 8],
        }
    }
}

pub fn window_state(window: &Window) -> RefMut<'_, WindowState> {
    let data = window.user_data();
    data.insert_if_missing(|| RefCell::new(WindowState::default()));
    data.get::<RefCell<WindowState>>().unwrap().borrow_mut()
}
