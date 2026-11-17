//! Exposé / Mission Control: every window of the current space (minimized
//! ones included) scaled down into a grid so you can pick one.

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle, Size},
};

use super::{window::window_state, Workspace};

const MARGIN: f64 = 48.0;
const GAP: f64 = 28.0;

/// Target rectangle (global logical coordinates) for each window.
pub fn targets(ws: &Workspace, area: Rectangle<f64, Logical>) -> Vec<(Window, Rectangle<f64, Logical>)> {
    // Stable order: by on-screen position, so the grid resembles the desktop.
    let mut windows: Vec<(Window, Point<f64, Logical>)> = ws
        .space
        .elements()
        .filter_map(|w| Some((w.clone(), ws.space.element_location(w)?.to_f64())))
        .chain(ws.minimized.iter().map(|w| (w.clone(), window_state(w).restore_loc.to_f64())))
        .filter(|(w, _)| !w.geometry().size.is_empty())
        .collect();
    windows.sort_by(|a, b| (a.1.y, a.1.x).partial_cmp(&(b.1.y, b.1.x)).unwrap());

    let n = windows.len();
    if n == 0 {
        return Vec::new();
    }
    let inner = Rectangle::new(
        area.loc + Point::from((MARGIN, MARGIN)),
        Size::from(((area.size.w - 2.0 * MARGIN).max(1.0), (area.size.h - 2.0 * MARGIN).max(1.0))),
    );
    // Pick the column count that makes cells closest to the screen's shape.
    let cols = (1..=n)
        .min_by(|&a, &b| {
            let score = |c: usize| {
                let rows = n.div_ceil(c) as f64;
                let cell = (inner.size.w / c as f64) / (inner.size.h / rows);
                (cell - 1.4).abs()
            };
            score(a).partial_cmp(&score(b)).unwrap()
        })
        .unwrap();
    let rows = n.div_ceil(cols);
    let cell = Size::<f64, Logical>::from((inner.size.w / cols as f64, inner.size.h / rows as f64));

    windows
        .into_iter()
        .enumerate()
        .map(|(i, (window, _))| {
            let (row, col) = (i / cols, i % cols);
            // Centre the last, possibly shorter, row.
            let in_row = if row == rows - 1 { n - row * cols } else { cols };
            let row_offset = (cols - in_row) as f64 * cell.w / 2.0;
            let size = window.geometry().size.to_f64();
            let fit = ((cell.w - GAP) / size.w).min((cell.h - GAP) / size.h).min(1.0);
            let scaled = size.upscale(fit);
            let loc = Point::from((
                inner.loc.x + row_offset + col as f64 * cell.w + (cell.w - scaled.w) / 2.0,
                inner.loc.y + row as f64 * cell.h + (cell.h - scaled.h) / 2.0,
            ));
            (window, Rectangle::new(loc, scaled))
        })
        .collect()
}
