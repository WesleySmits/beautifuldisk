//! The dithered usage bar and the pixel dust that rises off it while clearing.

use dither_gpui::dither_core::dither::hash01;
use dither_gpui::dither_core::{Rgba, Tone};
use dither_gpui::hsla;
use gpui_kit::{Bounds, Corners, Pixels, Window, fill, point, px, size};

use crate::ui::{s, snap, z};

/// Spacing of the dither grid and the size of each dot, in points.
const CELL: f32 = 5.0;
const DOT: f32 = 3.0;
const RADIUS: f32 = 7.0;
/// How far the leading-edge highlight reaches into a segment, in cells.
const EDGE_CELLS: f32 = 8.0;

#[derive(Clone)]
pub struct Segment {
    pub tone: Tone,
    /// Fractions of the bar width.
    pub start: f32,
    pub end: f32,
    /// 0 is dimmed, 1 is lit (hovered or working).
    pub glow: f32,
}

#[derive(Clone)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub color: Rgba,
}

#[derive(Clone)]
pub struct Bar {
    pub segments: Vec<Segment>,
    /// Segment whose right edge glows (where scanning or clearing happens).
    pub edge: Option<usize>,
    /// Strength of that glow, faded in and out by the card.
    pub edge_strength: f32,
    pub time: f32,
}

fn alpha(c: Rgba, a: f32) -> Rgba {
    c.with_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn paint(bar: &Bar, bounds: Bounds<Pixels>, window: &mut Window) {
    let (cell, dot_size) = (snap(CELL), snap(DOT));
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let origin = bounds.origin;
    let cols = (w / cell).floor() as i32;
    let rows = (h / cell).floor() as i32;
    let ox = (w - cols as f32 * cell) / 2.0 + (cell - dot_size) / 2.0;
    let oy = (h - rows as f32 * cell) / 2.0 + (cell - dot_size) / 2.0;
    let dot = |window: &mut Window, col: i32, row: i32, color: Rgba| {
        let at = point(
            origin.x + px(ox + col as f32 * cell),
            origin.y + px(oy + row as f32 * cell),
        );
        window.paint_quad(fill(
            Bounds::new(at, size(px(dot_size), px(dot_size))),
            hsla(color),
        ));
    };

    window.paint_quad(
        fill(bounds, hsla(Rgba::hex(0x1b1b1e)))
            .corner_radii(z(RADIUS))
            .border_widths(px(1.0))
            .border_color(hsla(alpha(Rgba::WHITE, 0.06))),
    );

    // Empty track: a faint grid of dots past the used space.
    let used_end = bar.segments.last().map_or(0.0, |s| s.end) * w;
    let faint = alpha(Rgba::WHITE, 0.045);
    for col in 0..cols {
        if ox + col as f32 * cell + dot_size / 2.0 < used_end {
            continue;
        }
        for row in 0..rows {
            dot(window, col, row, faint);
        }
    }

    let flicker = (bar.time * 20.0) as i32;
    for (i, seg) in bar.segments.iter().enumerate() {
        let x0 = seg.start * w;
        let x1 = seg.end * w;
        if x1 - x0 < 0.5 {
            continue;
        }
        let glow = seg.glow;
        let base = seg.tone.base();
        let first = i == 0;
        let radius = |on: bool| px(if on { s(RADIUS) } else { s(1.5) });
        window.paint_quad(
            fill(
                Bounds::new(
                    origin + point(px(x0), px(0.0)),
                    size(px(x1 - x0), bounds.size.height),
                ),
                hsla(alpha(seg.tone.shade(0.28), 0.22 + 0.3 * glow)),
            )
            .corner_radii(Corners {
                top_left: radius(first),
                bottom_left: radius(first),
                top_right: radius(false),
                bottom_right: radius(false),
            })
            .border_widths(px(1.0))
            .border_color(hsla(alpha(base, 0.35 + 0.65 * glow))),
        );

        let edge = if bar.edge == Some(i) {
            bar.edge_strength
        } else {
            0.0
        };
        for col in 0..cols {
            let cx = ox + col as f32 * cell + dot_size / 2.0;
            if cx < x0 + 1.0 || cx > x1 - 1.0 {
                continue;
            }
            // 1 at the segment's right edge, fading to 0 EDGE_CELLS inside it.
            let near = (1.0 - (x1 - cx) / (cell * EDGE_CELLS)).max(0.0) * edge;
            for row in 0..rows {
                let fy = row as f32 / (rows - 1).max(1) as f32;
                let density =
                    (0.62 - 0.5 * fy) * (0.3 + 0.7 * glow) + 0.28 * glow * glow + near * 0.9;
                let seed = if near > 0.0 { row + flicker * 131 } else { row };
                if hash01(col, seed) >= density {
                    continue;
                }
                let sparkle = hash01(col ^ 0x2f, row);
                let color = seg
                    .tone
                    .shade(0.6 + 0.3 * sparkle * glow)
                    .mix(Rgba::WHITE, near * 0.85);
                dot(window, col, row, alpha(color, 0.4 + 0.6 * glow.max(near)));
            }
        }

        // Bloom spilling past the leading edge.
        if edge > 0.01 {
            let first_col = ((x1 - ox) / cell).ceil() as i32;
            for k in 0..3 {
                let col = first_col + k;
                if col >= cols {
                    break;
                }
                let fade = edge * 0.45 * (1.0 - k as f32 / 3.0);
                for row in 0..rows {
                    if hash01(col, row + flicker * 131) < fade {
                        dot(window, col, row, alpha(seg.tone.shade(0.8), fade + 0.2));
                    }
                }
            }
        }
    }
}

/// Paints particles; `origin` is the bar's top-left corner.
pub fn paint_particles(
    particles: &[Particle],
    origin: gpui_kit::Point<Pixels>,
    window: &mut Window,
) {
    for p in particles {
        let t = (p.life / p.max_life).clamp(0.0, 1.0);
        let side = s(1.5 + 1.5 * t);
        let at = origin + point(px(p.x - side / 2.0), px(p.y - side / 2.0));
        window.paint_quad(fill(
            Bounds::new(at, size(px(side), px(side))),
            hsla(alpha(p.color, t * 0.95)),
        ));
    }
}

/// Small xorshift RNG so particle spawning needs no dependency.
pub struct Rng(u32);

impl Rng {
    pub fn new() -> Self {
        Self(0x9e37_79b9)
    }

    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Advances particles and emits new ones from each rectangle (left, top,
/// right, bottom in points) being cleared, at `rate` per second each.
pub fn step_particles(
    particles: &mut Vec<Particle>,
    rng: &mut Rng,
    dt: f32,
    emit: &[([f32; 4], Tone)],
    rate: f32,
) {
    for p in particles.iter_mut() {
        p.x += p.vx * dt;
        p.y += p.vy * dt;
        p.vy -= 45.0 * dt;
        p.vx *= 1.0 - 0.8 * dt;
        p.life -= dt;
    }
    particles.retain(|p| p.life > 0.0);

    for &([x0, y0, x1, y1], tone) in emit {
        let count = (rate * dt + rng.next()).floor() as usize;
        for _ in 0..count {
            let max_life = rng.range(0.45, 1.1);
            let white = rng.next();
            particles.push(Particle {
                x: rng.range(x0, x1),
                y: rng.range(y0, y1),
                vx: rng.range(-14.0, 30.0),
                vy: rng.range(-75.0, -15.0),
                life: max_life,
                max_life,
                color: tone
                    .shade(rng.range(0.6, 0.85))
                    .mix(Rgba::WHITE, if white > 0.7 { 0.7 } else { 0.0 }),
            });
        }
    }
}
