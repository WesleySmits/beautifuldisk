//! Tiles for "Your files": each file is drawn as dithered dots on the
//! treemap's grid, shaded by its own picture, and can develop into that
//! picture (every cell filled with its real color) or crumble away.

use dither_gpui::dither_core::dither::hash01;
use dither_gpui::dither_core::{Rgba, Tone};
use dither_gpui::hsla;
use gpui_kit::{Bounds, Pixels, Point, Window, fill, point, px, size};

use crate::files::Thumb;
use crate::treemap::CELL;
use crate::ui::{s, snap};

const DOT: f32 = 3.0;
const RADIUS: f32 = 7.0;

const BAYER: [f32; 16] = [
    0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
];

pub struct Tile {
    pub tone: Tone,
    /// Picked to go to the Trash.
    pub on: bool,
    /// 0 dim, 1 lit (hovered).
    pub glow: f32,
    /// 0 dots, 1 the picture itself.
    pub develop: f32,
    /// Share of the dots gone, as the file crumbles away.
    pub dissolve: f32,
    /// How much has filled in, from the bottom up, as tiles arrive.
    pub reveal: f32,
    /// Where a band of light crosses, in the grid's x, while files move to
    /// the Trash.
    pub band: Option<f32>,
}

fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let k = k.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
    Rgba {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: m(a.a, b.a),
    }
}

fn alpha(c: Rgba, a: f32) -> Rgba {
    c.with_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Paints one tile at `bounds`. `grid` is the area's origin, so neighboring
/// tiles share one dot grid.
pub fn paint(
    tile: &Tile,
    thumb: Option<&Thumb>,
    bounds: Bounds<Pixels>,
    grid: Point<Pixels>,
    time: f32,
    window: &mut Window,
) {
    let (cell, dot_size) = (snap(CELL), snap(DOT));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w < 2.0 || h < 2.0 {
        return;
    }
    let glow = tile.glow;
    let solid = 1.0 - tile.dissolve;
    let (bg, border) = if tile.on {
        (
            alpha(tile.tone.shade(0.25), (0.18 + 0.12 * glow) * solid),
            alpha(tile.tone.base(), (0.45 + 0.55 * glow) * solid),
        )
    } else {
        (
            alpha(Rgba::hex(0x1b1b1e), solid),
            alpha(Rgba::WHITE, (0.05 + 0.1 * glow) * solid),
        )
    };
    window.paint_quad(
        fill(bounds, hsla(bg))
            .corner_radii(px(s(RADIUS).min(w / 2.0).min(h / 2.0)))
            .border_widths(px(1.0))
            .border_color(hsla(border)),
    );

    let aspect = w / h;
    let (gx, gy) = (
        f32::from(bounds.origin.x - grid.x),
        f32::from(bounds.origin.y - grid.y),
    );
    let first_col = ((gx + s(2.0)) / cell).ceil() as i32;
    let last_col = ((gx + w - s(2.0) - dot_size) / cell).floor() as i32;
    let first_row = ((gy + s(2.0)) / cell).ceil() as i32;
    let last_row = ((gy + h - s(2.0) - dot_size) / cell).floor() as i32;
    let flicker = (time * 14.0) as i32;
    // With no picture to develop into (Linux, or one that failed to load),
    // the dots only thicken, or the tile would turn into a flat block.
    let develop = tile.develop.clamp(0.0, 1.0) * if thumb.is_some() { 1.0 } else { 0.4 };
    let dot = dot_size + (cell - dot_size) * develop;
    for row in first_row..=last_row {
        let v = ((row as f32 * cell - gy) / h).clamp(0.0, 1.0);
        if v < 1.0 - tile.reveal {
            continue;
        }
        for col in first_col..=last_col {
            if hash01(col, row) < tile.dissolve {
                continue;
            }
            let u = ((col as f32 * cell - gx) / w).clamp(0.0, 1.0);
            let real = thumb.map(|t| t.at(u, v, aspect));
            // Brighter parts of the picture get more dots. Without one, the
            // dots thicken toward the bottom like the cache tiles.
            let light = match real {
                Some([r, g, b]) => {
                    0.12 + 0.88 * (r as f32 * 0.3 + g as f32 * 0.59 + b as f32 * 0.11) / 255.0
                }
                None => 0.1 + 0.82 * v.powf(1.3) * (0.6 + 0.4 * u),
            };
            let near = tile.band.map_or(0.0, |x| {
                (-((col as f32 * cell - x) / s(28.0)).powi(2)).exp()
            });
            let mut density = if tile.on {
                light * (0.55 + 0.45 * glow)
            } else {
                light * (0.3 + 0.3 * glow)
            } + near * 0.5;
            if tile.band.is_some() && hash01(col, row + flicker * 131) < 0.2 {
                density += 0.3;
            }
            let threshold = (BAYER[((row & 3) * 4 + (col & 3)) as usize] + 0.5) / 16.0;
            if density <= threshold * (1.0 - develop) {
                continue;
            }
            let dots = if tile.on {
                let sparkle = hash01(col ^ 0x2f, row);
                alpha(
                    tile.tone.shade(0.6 + 0.3 * sparkle * glow),
                    0.55 + 0.45 * glow,
                )
            } else {
                alpha(Rgba::WHITE, 0.16 + 0.14 * glow)
            };
            let picture = real.map_or(dots, |[r, g, b]| Rgba { r, g, b, a: 255 });
            let color = mix(mix(dots, picture, develop), Rgba::WHITE, near * 0.7);
            let at = grid + point(px(col as f32 * cell), px(row as f32 * cell));
            window.paint_quad(fill(Bounds::new(at, size(px(dot), px(dot))), hsla(color)));
        }
    }
}
