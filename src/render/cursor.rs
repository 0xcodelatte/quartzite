//! Software cursor for the DRM backend (the nested backend uses the host's).

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::render_elements_from_surface_tree,
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    utils::{Logical, Point, Scale, Transform},
    wayland::compositor::with_states,
};

use super::OutputElement;

pub struct Cursor {
    pub status: CursorImageStatus,
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
}

impl Cursor {
    pub fn load() -> Self {
        let (buffer, hotspot) = load_xcursor().unwrap_or_else(|| {
            tracing::warn!("no xcursor theme found, using built-in arrow");
            fallback_arrow()
        });
        Self { status: CursorImageStatus::default_named(), buffer, hotspot }
    }

    /// `pos` is the pointer position relative to the output, in logical px.
    pub fn render_elements(
        &self,
        renderer: &mut GlesRenderer,
        pos: Point<f64, Logical>,
        scale: f64,
    ) -> Vec<OutputElement> {
        match &self.status {
            CursorImageStatus::Hidden => Vec::new(),
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|d| d.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                let loc = (pos - hotspot.to_f64()).to_physical_precise_round(scale);
                render_elements_from_surface_tree(renderer, surface, loc, Scale::from(scale), 1.0, Kind::Cursor)
                    .into_iter()
                    .map(OutputElement::Surface)
                    .collect()
            }
            // Named cursors all use the theme's default arrow for now.
            CursorImageStatus::Named(_) => {
                let loc = (pos - self.hotspot.to_f64()).to_physical(scale);
                MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &self.buffer, None, None, None, Kind::Cursor)
                    .map(|e| vec![OutputElement::Cursor(e)])
                    .unwrap_or_default()
            }
        }
    }
}

fn load_xcursor() -> Option<(MemoryRenderBuffer, Point<i32, Logical>)> {
    let theme_name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
    let size: u32 = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
    let theme = xcursor::CursorTheme::load(&theme_name);
    let path = theme.load_icon("default").or_else(|| theme.load_icon("left_ptr"))?;
    let data = std::fs::read(path).ok()?;
    let images = xcursor::parser::parse_xcursor(&data)?;
    let image = images.iter().min_by_key(|i| (i.size as i32 - size as i32).abs())?;
    let buffer = MemoryRenderBuffer::from_slice(
        &image.pixels_rgba,
        Fourcc::Abgr8888,
        (image.width as i32, image.height as i32),
        1,
        Transform::Normal,
        None,
    );
    Some((buffer, (image.xhot as i32, image.yhot as i32).into()))
}

/// A 16x24 white arrow with a black outline, rasterised at startup.
fn fallback_arrow() -> (MemoryRenderBuffer, Point<i32, Logical>) {
    let (w, h) = (16usize, 24usize);
    let inside = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && y <= 20.0 && x <= y * 0.62 && !(y > 14.0 && x > 7.0);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * w + x) * 4;
            if inside(fx - 1.0, fy - 1.0) {
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if inside(fx - 1.0, fy - 0.0) || inside(fx, fy - 1.0) || inside(fx - 2.0, fy - 1.0) || inside(fx - 1.0, fy - 2.0) {
                px[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    let buffer = MemoryRenderBuffer::from_slice(&px, Fourcc::Abgr8888, (w as i32, h as i32), 1, Transform::Normal, None);
    (buffer, (1, 1).into())
}
