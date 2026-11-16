//! The cleanup treemap: one dithered tile per item, sized by its bytes.

use dither_gpui::dither_core::dither::hash01;
use dither_gpui::dither_core::{Rgba, Tone};
use dither_gpui::hsla;
use gpui_kit::{Bounds, Pixels, Window, fill, point, px, size};

/// Same dot grid as the usage bar, so the two read as one surface.
/// Finer than the usage bar: tiles are big, and dense pixels read as a
/// gradient instead of scattered dots.
pub const CELL: f32 = 4.0;
const DOT: f32 = 3.0;
const RADIUS: f32 = 7.0;
/// Space between tiles.
pub const GAP: f32 = 4.0;

#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Tiles for every item. The shape of the split comes from `initial` (the
/// sizes when the items were found), so tiles never jump to new places;
/// only their proportions follow `weight`, so a shrinking item gives its
/// room to its neighbours.
pub fn layout(initial: &[f64], weight: &[f64], area: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::default(); initial.len()];
    let ids: Vec<usize> = (0..initial.len()).collect();
    split(&ids, initial, weight, area, area, &mut out);
    out
}

fn split(ids: &[usize], initial: &[f64], weight: &[f64], shape: Rect, at: Rect, out: &mut [Rect]) {
    let [only] = ids else {
        if ids.is_empty() {
            return;
        }
        // Halve by initial size, largest items first.
        let total: f64 = ids.iter().map(|&i| initial[i]).sum();
        let mut k = 1;
        let mut acc = initial[ids[0]];
        while k < ids.len() - 1 && acc + initial[ids[k]] < total / 2.0 {
            acc += initial[ids[k]];
            k += 1;
        }
        let (a, b) = ids.split_at(k);
        let sum = |ids: &[usize], w: &[f64]| ids.iter().map(|&i| w[i]).sum::<f64>();
        let shape_f = (acc / total.max(1.0)) as f32;
        let (wa, wb) = (sum(a, weight), sum(b, weight));
        let f = if wa + wb > 0.0 {
            (wa / (wa + wb)) as f32
        } else {
            shape_f
        };
        // Split the way the initial layout did, so orientation is stable.
        if shape.w >= shape.h {
            let (sa, sb) = (shape.w * shape_f, shape.w * (1.0 - shape_f));
            split(
                a,
                initial,
                weight,
                Rect { w: sa, ..shape },
                Rect { w: at.w * f, ..at },
                out,
            );
            split(
                b,
                initial,
                weight,
                Rect {
                    x: shape.x + sa,
                    w: sb,
                    ..shape
                },
                Rect {
                    x: at.x + at.w * f,
                    w: at.w * (1.0 - f),
                    ..at
                },
                out,
            );
        } else {
            let (sa, sb) = (shape.h * shape_f, shape.h * (1.0 - shape_f));
            split(
                a,
                initial,
                weight,
                Rect { h: sa, ..shape },
                Rect { h: at.h * f, ..at },
                out,
            );
            split(
                b,
                initial,
                weight,
                Rect {
                    y: shape.y + sa,
                    h: sb,
                    ..shape
                },
                Rect {
                    y: at.y + at.h * f,
                    h: at.h * (1.0 - f),
                    ..at
                },
                out,
            );
        }
        return;
    };
    out[*only] = at;
}

pub struct Tile {
    pub tone: Tone,
    pub on: bool,
    /// 0 dim, 1 lit (hovered or being cleared).
    pub glow: f32,
    /// How much of the tile has filled in, from the bottom up.
    pub reveal: f32,
    /// Being cleared: the dots shimmer.
    pub busy: bool,
}

const BAYER: [f32; 16] = [
    0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
];

fn alpha(c: Rgba, a: f32) -> Rgba {
    c.with_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Paints one tile. `grid` is the treemap's origin, so every tile's dots sit
/// on one shared grid.
pub fn paint(
    tile: &Tile,
    bounds: Bounds<Pixels>,
    grid: gpui_kit::Point<Pixels>,
    time: f32,
    window: &mut Window,
) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w < 2.0 || h < 2.0 {
        return;
    }
    let base = tile.tone.base();
    let glow = tile.glow;
    let (bg, border) = if tile.on {
        (
            alpha(tile.tone.shade(0.25), 0.18 + 0.12 * glow),
            alpha(base, 0.45 + 0.55 * glow),
        )
    } else {
        (Rgba::hex(0x1b1b1e), alpha(Rgba::WHITE, 0.05 + 0.08 * glow))
    };
    window.paint_quad(
        fill(bounds, hsla(bg))
            .corner_radii(px(RADIUS.min(w / 2.0).min(h / 2.0)))
            .border_widths(px(1.0))
            .border_color(hsla(border)),
    );

    let (gx, gy) = (
        f32::from(bounds.origin.x - grid.x),
        f32::from(bounds.origin.y - grid.y),
    );
    let first_col = ((gx + 2.0) / CELL).ceil() as i32;
    let last_col = ((gx + w - 2.0 - DOT) / CELL).floor() as i32;
    let first_row = ((gy + 2.0) / CELL).ceil() as i32;
    let last_row = ((gy + h - 2.0 - DOT) / CELL).floor() as i32;
    let flicker = (time * 14.0) as i32;
    for row in first_row..=last_row {
        let v = ((row as f32 * CELL - gy) / h).clamp(0.0, 1.0);
        if v < 1.0 - tile.reveal {
            continue;
        }
        for col in first_col..=last_col {
            let u = ((col as f32 * CELL - gx) / w).clamp(0.0, 1.0);
            let threshold = (BAYER[((row & 3) * 4 + (col & 3)) as usize] + 0.5) / 16.0;
            let color = if tile.on {
                // Sparse at the top where the label sits, dense at the bottom.
                let mut density = (0.1 + 0.82 * v.powf(1.3) * (0.6 + 0.4 * u)) * (0.6 + 0.4 * glow);
                if tile.busy && hash01(col, row + flicker * 131) < 0.25 {
                    density += 0.5;
                }
                if density <= threshold {
                    continue;
                }
                let sparkle = hash01(col ^ 0x2f, row);
                alpha(
                    tile.tone.shade(0.6 + 0.3 * sparkle * glow),
                    0.55 + 0.45 * glow,
                )
            } else {
                if (col + row) % 4 != 0 {
                    continue;
                }
                alpha(Rgba::WHITE, 0.13 + 0.1 * glow)
            };
            let at = grid + point(px(col as f32 * CELL), px(row as f32 * CELL));
            window.paint_quad(fill(Bounds::new(at, size(px(DOT), px(DOT))), hsla(color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 600.0,
        h: 300.0,
    };

    fn area(r: &Rect) -> f32 {
        r.w * r.h
    }

    #[test]
    fn tiles_fill_the_area_in_proportion() {
        let sizes = [30.0, 16.0, 10.0, 3.0, 1.0];
        let tiles = layout(&sizes, &sizes, AREA);
        let total: f32 = tiles.iter().map(area).sum();
        assert!((total - area(&AREA)).abs() < 1.0);
        for (t, s) in tiles.iter().zip(sizes) {
            let share = area(t) / area(&AREA);
            assert!((share - (s / 60.0) as f32).abs() < 1e-3);
        }
    }

    #[test]
    fn a_cleared_item_gives_its_room_without_moving_others() {
        let sizes = [30.0, 16.0, 10.0, 3.0, 1.0];
        let before = layout(&sizes, &sizes, AREA);
        let after = layout(&sizes, &[30.0, 0.0, 10.0, 3.0, 1.0], AREA);
        assert_eq!(area(&after[1]), 0.0);
        let total: f32 = after.iter().map(area).sum();
        assert!((total - area(&AREA)).abs() < 1.0);
        // The biggest tile keeps its corner.
        assert_eq!((after[0].x, after[0].y), (before[0].x, before[0].y));
    }
}
