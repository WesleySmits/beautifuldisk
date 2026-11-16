use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use dither_gpui::dither_core::Tone;
use dither_gpui::dither_core::ease::approach;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::bar::{self, Bar, Particle, Rng, Segment};
use crate::clutter::{self, Item};
use crate::disk::{self, Category, Progress, Report, Volume};
use crate::sound::{Sound, Ticker};
use crate::treemap::{self, Rect};

const BG: u32 = 0x161618;
const FG: u32 = 0xededef;
const MUTED: u32 = 0x8b8b90;
const LINE: u32 = 0x26262a;
const OK: u32 = 0x4ccb76;

fn tone(category: Category) -> Tone {
    match category {
        Category::Apps => Tone::Blue,
        Category::Developer => Tone::Green,
        Category::Photos => Tone::Pink,
        Category::Movies => Tone::Purple,
        Category::Documents => Tone::Hue(205.0),
        Category::Music => Tone::Hue(95.0),
        Category::System => Tone::Grey,
        Category::Caches => Tone::Orange,
        Category::Trash => Tone::Red,
    }
}

fn icon(category: Category) -> IconName {
    match category {
        Category::Apps => IconName::AppWindow,
        Category::Developer => IconName::SquareTerminal,
        Category::Photos => IconName::Image,
        Category::Movies => IconName::Film,
        Category::Documents => IconName::FileText,
        Category::Music => IconName::Music,
        Category::System => IconName::Settings,
        Category::Caches => IconName::Package,
        Category::Trash => IconName::Trash,
    }
}

fn gb(bytes: f64) -> String {
    // Easing can dip a hair below zero; never show "-0.0 GB".
    format!("{:.1} GB", (bytes / 1e9).max(0.0) + 0.0)
}

/// Disk capacity reads like a product spec: "512 GB", "1 TB".
fn capacity(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{} TB", (gb / 100.0).round() / 10.0)
    } else {
        format!("{} GB", gb.round())
    }
}

fn tilde(path: &str) -> String {
    let home = disk::home();
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if home != "/" => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Scanning,
    Idle,
    Clearing,
}

pub struct Card {
    demo: bool,
    autoplay: bool,
    volume: Volume,
    progress: Arc<Progress>,
    phase: Phase,
    report: Report,
    /// Bytes per category: where the worker says they are, and what is on screen.
    target: [f64; 9],
    shown: [f64; 9],
    glow: [f32; 9],
    edge_strength: f32,
    hover: Option<Category>,
    freed: u64,
    /// Cleanup items once detection finishes, with what the UI tracks per item.
    items: Option<Arc<Vec<Item>>>,
    selected: Vec<bool>,
    /// Bytes on screen per item, easing toward what is left of it.
    item_shown: Vec<f64>,
    item_glow: Vec<f32>,
    /// Each item's size when found: fixes the treemap's shape.
    item_size: Vec<f64>,
    /// Size of all items when found.
    items_total: f64,
    hover_item: Option<usize>,
    /// When the items arrived, for their staggered sweep in.
    items_at: f32,
    clean_bounds: Rc<Cell<Bounds<Pixels>>>,
    particles: Vec<Particle>,
    rng: Rng,
    time: f32,
    last_frame: Instant,
    /// Whether the previous frame asked for another. If not, the time since
    /// it is idle time, not animation time.
    animating: bool,
    bar_bounds: Rc<Cell<Bounds<Pixels>>>,
    legend_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The legend's hover highlight glides between rows: x, y, width relative
    /// to the legend, and opacity.
    pill: [f32; 3],
    pill_velocity: [f32; 3],
    pill_alpha: f32,
    sound: Sound,
    ticker: Ticker,
    /// The item being cleared last frame, to whoosh when it is done.
    was_cleaning: u64,
}

/// Legend geometry, shared by the layout and the gliding highlight.
const ROW_H: f32 = 22.0;
const ROW_GAP: f32 = 2.0;
const COL_GAP: f32 = 12.0;

/// Stiffness of the highlight's glide. Settles in about 0.2s.
const PILL_OMEGA: f32 = 30.0;

/// One step of a critically damped spring, solved exactly so any frame time
/// is stable. `x` is the offset from the target. Keeping velocity across
/// steps means a new target mid-glide bends the path instead of jerking it.
fn spring(x: f32, v: f32, dt: f32) -> (f32, f32) {
    let w = PILL_OMEGA;
    let c = v + w * x;
    let e = (-w * dt).exp();
    ((x + c * dt) * e, (v - w * c * dt) * e)
}

impl Card {
    pub fn new(demo: bool, autoplay: bool) -> Self {
        let mut card = Self {
            demo,
            autoplay,
            volume: Volume { total: 0, used: 0 },
            progress: Arc::default(),
            phase: Phase::Idle,
            report: Report::default(),
            target: [0.0; 9],
            shown: [0.0; 9],
            glow: [1.0; 9],
            edge_strength: 0.0,
            hover: None,
            freed: 0,
            items: None,
            selected: Vec::new(),
            item_shown: Vec::new(),
            item_glow: Vec::new(),
            item_size: Vec::new(),
            items_total: 0.0,
            hover_item: None,
            items_at: 0.0,
            clean_bounds: Rc::default(),
            particles: Vec::new(),
            rng: Rng::new(),
            time: 0.0,
            last_frame: Instant::now(),
            animating: false,
            bar_bounds: Rc::default(),
            legend_bounds: Rc::default(),
            pill: [0.0; 3],
            pill_velocity: [0.0; 3],
            pill_alpha: 0.0,
            sound: Sound::new(),
            ticker: Ticker::default(),
            was_cleaning: 0,
        };
        // Autoplay waits a moment so a screen recording catches the scan.
        if !autoplay {
            card.scan();
        }
        card
    }

    fn scan(&mut self) {
        self.volume = if self.demo {
            disk::demo::VOLUME
        } else {
            disk::volume()
        };
        self.progress = Arc::default();
        self.phase = Phase::Scanning;
        self.report = Report::default();
        self.target = [0.0; 9];
        self.shown = [0.0; 9];
        self.hover = None;
        self.hover_item = None;
        self.items = None;
        self.item_shown.clear();
        self.freed = 0;
        if self.demo {
            disk::demo::scan(self.progress.clone());
        } else {
            disk::scan(self.progress.clone());
        }
    }

    fn clear(&mut self) {
        let Some(items) = self.items.clone() else {
            return;
        };
        let chosen: Vec<usize> = (0..items.len())
            .filter(|&i| self.selected[i] && items[i].left() > 0)
            .collect();
        if chosen.is_empty() {
            return;
        }
        self.phase = Phase::Clearing;
        self.hover = None;
        self.hover_item = None;
        clutter::clean(items, chosen, self.progress.clone(), self.demo);
    }

    /// Items still worth a chip: everything until cleared away.
    fn visible(&self) -> Vec<usize> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        (0..items.len())
            .filter(|&i| items[i].left() > 0 || self.item_shown[i] > 1e6)
            .collect()
    }

    fn selected_bytes(&self) -> f64 {
        self.visible()
            .into_iter()
            .filter(|&i| self.selected[i])
            .map(|i| self.item_shown[i])
            .sum()
    }

    /// How full the cleanup bar is on screen, 0 to 1.
    fn items_fill(&self) -> f64 {
        self.item_shown.iter().sum::<f64>() / self.items_total.max(1.0)
    }

    fn used_shown(&self) -> f64 {
        self.shown.iter().sum()
    }

    /// Samples the worker, eases every animated value toward its target and
    /// says whether another frame is needed.
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        // The first frame after idle (a hover starting a glide) would see
        // seconds of elapsed time; use one display frame so it starts smooth.
        let dt = if self.animating {
            (now - self.last_frame).as_secs_f32().min(1.0 / 20.0)
        } else {
            1.0 / 120.0
        };
        self.last_frame = now;
        self.time += dt;

        let working = self.phase != Phase::Idle;
        if working {
            for c in Category::ALL {
                if c != Category::System {
                    self.target[c as usize] = self.progress.bytes(c) as f64;
                }
            }
            self.freed = self.progress.freed();
        }
        if self.progress.is_done() {
            match self.phase {
                Phase::Scanning => {
                    self.report = self.progress.report();
                    let known: f64 = self.target.iter().sum();
                    self.target[Category::System as usize] =
                        (self.volume.used as f64 - known).max(0.0);
                    self.phase = Phase::Idle;
                }
                Phase::Clearing => {
                    // What was cleared is gone; what was kept stays, unselected.
                    if let Some(items) = &self.items {
                        for (i, item) in items.iter().enumerate() {
                            if self.selected[i] && item.left() > 0 {
                                self.selected[i] = false;
                            }
                        }
                    }
                    self.phase = Phase::Idle;
                }
                Phase::Idle => {}
            }
        }

        let total = (self.volume.total as f64).max(1.0);
        let used_before = self.used_shown() / total;
        let items_before = self.items_fill();

        let mut moving = false;
        for i in 0..9 {
            let (from, to) = (self.shown[i], self.target[i]);
            self.shown[i] = approach(from as f32, to as f32, dt, 0.09) as f64;
            if (self.shown[i] - to).abs() < 5e6 {
                self.shown[i] = to;
            } else {
                moving = true;
            }

            let category = Category::ALL[i];
            let glow_to = match (self.phase, self.hover) {
                (Phase::Idle, Some(h)) if h == category => 1.0,
                (Phase::Idle, Some(_)) => 0.15,
                (Phase::Idle, None) => 0.45,
                _ => 1.0,
            };
            self.glow[i] = approach(self.glow[i], glow_to, dt, 0.07);
            if (self.glow[i] - glow_to).abs() > 0.005 {
                moving = true;
            } else {
                self.glow[i] = glow_to;
            }
        }

        // Gliding highlight: slide to the hovered row; when nothing is
        // hovered, fade out in place. Appearing from nothing jumps there
        // first so it never slides in from a stale spot.
        let legend_w = f32::from(self.legend_bounds.get().size.width);
        let hovered = self
            .hover
            .filter(|_| self.phase == Phase::Idle && legend_w > 0.0);
        if let Some(c) = hovered {
            let i = c as usize;
            let w = (legend_w - 2.0 * COL_GAP) / 3.0;
            let to = [
                (i % 3) as f32 * (w + COL_GAP),
                (i / 3) as f32 * (ROW_H + ROW_GAP),
                w,
            ];
            for k in 0..3 {
                if self.pill_alpha < 0.02 {
                    self.pill[k] = to[k];
                    self.pill_velocity[k] = 0.0;
                    continue;
                }
                let (x, v) = spring(self.pill[k] - to[k], self.pill_velocity[k], dt);
                if x.abs() > 0.05 || v.abs() > 1.0 {
                    self.pill[k] = to[k] + x;
                    self.pill_velocity[k] = v;
                    moving = true;
                } else {
                    self.pill[k] = to[k];
                    self.pill_velocity[k] = 0.0;
                }
            }
        }
        let alpha_to = if hovered.is_some() { 1.0 } else { 0.0 };
        self.pill_alpha = approach(self.pill_alpha, alpha_to, dt, 0.06);
        if (self.pill_alpha - alpha_to).abs() > 0.005 {
            moving = true;
        } else {
            self.pill_alpha = alpha_to;
        }

        if self.items.is_none()
            && self.phase == Phase::Idle
            && let Some(items) = self.progress.items()
        {
            self.selected = items.iter().map(|i| i.on).collect();
            self.item_shown = vec![0.0; items.len()];
            self.item_glow = vec![0.0; items.len()];
            self.item_size = items.iter().map(|i| i.left() as f64).collect();
            self.items_total = self.item_size.iter().sum();
            self.items_at = self.time;
            self.items = Some(items);
        }
        if self.autoplay && self.play_script() {
            moving = true;
        }
        let cleaning = self.progress.cleaning();
        if let Some(items) = &self.items {
            for (i, item) in items.iter().enumerate() {
                let to = item.left() as f64;
                // Items sweep in one after another, left to right.
                let waiting = self.time - self.items_at < i as f32 * 0.035;
                if waiting {
                    moving = true;
                } else if self.item_shown[i] != to {
                    self.item_shown[i] =
                        approach(self.item_shown[i] as f32, to as f32, dt, 0.08) as f64;
                    if (self.item_shown[i] - to).abs() < 2e6 {
                        self.item_shown[i] = to;
                    } else {
                        moving = true;
                    }
                }
                let glow_to = match (self.hover_item, busy(cleaning, i)) {
                    (_, true) => 1.0,
                    (Some(h), _) if h == i && !self.selected[i] => 0.6,
                    _ if !self.selected[i] => 0.06,
                    (Some(h), _) if h == i => 1.0,
                    (Some(_), _) => 0.3,
                    _ => 0.62,
                };
                self.item_glow[i] = approach(self.item_glow[i], glow_to, dt, 0.06);
                if (self.item_glow[i] - glow_to).abs() > 0.005 {
                    moving = true;
                } else {
                    self.item_glow[i] = glow_to;
                }
            }
        }
        // One ticker for both bars, following whichever moved more, so
        // clearing (which shrinks both) does not tick twice.
        let used_after = self.used_shown() / total;
        let items_after = self.items_fill();
        let (delta, fill) = if (items_after - items_before).abs() > (used_after - used_before).abs()
        {
            (items_after - items_before, items_after)
        } else {
            (used_after - used_before, used_after)
        };
        self.ticker.step(&self.sound, delta as f32, dt, fill as f32);
        // An item that just finished.
        if self.was_cleaning & !cleaning != 0 {
            self.sound.whoosh();
        }
        self.was_cleaning = cleaning;

        // Keep frames coming while detection runs, so its result shows up.
        if self.items.is_none() && self.phase == Phase::Idle {
            moving = true;
        }

        let edge_to = if working { 1.0 } else { 0.0 };
        self.edge_strength = approach(self.edge_strength, edge_to, dt, 0.12);
        if (self.edge_strength - edge_to).abs() > 0.005 {
            moving = true;
        }

        // Dust rises off every tile being cleared.
        let mut emit = Vec::new();
        if cleaning != 0
            && let Some(items) = &self.items
        {
            for (i, t) in self.tiles().into_iter().enumerate() {
                if busy(cleaning, i) && t.w > 2.0 && t.h > 2.0 {
                    emit.push(([t.x, t.y, t.x + t.w, t.y + t.h], items[i].tone));
                }
            }
        }
        // Spread the dust so many tiles at once don't flood the screen.
        let rate = 260.0 / (emit.len().max(1) as f32).sqrt();
        bar::step_particles(&mut self.particles, &mut self.rng, dt, &emit, rate);

        working || moving || !self.particles.is_empty()
    }

    /// `--autoplay`: scans after a pause, hovers a few items once they are
    /// in, then clears. Returns whether the script is still waiting to act.
    fn play_script(&mut self) -> bool {
        const HOVERS: [usize; 3] = [0, 3, 1];
        const HOLD: f32 = 0.6;
        if self.volume.total == 0 {
            if self.time > 2.0 {
                self.scan();
            }
            return true;
        }
        if self.phase != Phase::Idle || self.items.is_none() || self.freed > 0 {
            return false;
        }
        let t = self.time - self.items_at - 1.4;
        if t < 0.0 {
            return true;
        }
        match HOVERS.get((t / HOLD) as usize) {
            Some(&i) => {
                self.hover_item = Some(i);
                true
            }
            None => {
                self.clear();
                false
            }
        }
    }

    /// What each tile's area follows: the item's size until its intro is
    /// done, then what is on screen, so a cleared tile shrinks away.
    fn weights(&self) -> Vec<f64> {
        let Some(items) = &self.items else {
            return Vec::new();
        };
        items
            .iter()
            .zip(&self.item_shown)
            .map(|(item, &shown)| shown.max(item.left() as f64))
            .collect()
    }

    /// Tile rectangles in the treemap's own coordinates.
    fn tiles(&self) -> Vec<Rect> {
        let size = self.clean_bounds.get().size;
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: f32::from(size.width),
            h: f32::from(size.height),
        };
        treemap::layout(&self.item_size, &self.weights(), area)
    }

    /// Start and end of a category's segment as fractions of the bar.
    fn span(&self, category: Category) -> (f32, f32) {
        let total = self.volume.total.max(1) as f64;
        let i = category as usize;
        let start: f64 = self.shown[..i].iter().sum();
        (
            (start / total) as f32,
            ((start + self.shown[i]) / total) as f32,
        )
    }

    fn bar(&self) -> Bar {
        let segments = Category::ALL
            .iter()
            .map(|&c| {
                let (start, end) = self.span(c);
                Segment {
                    tone: tone(c),
                    start,
                    end,
                    glow: self.glow[c as usize],
                }
            })
            .collect();
        let cleaning = self.progress.cleaning();
        let edge = match &self.items {
            Some(items) if cleaning != 0 => {
                Some(items[cleaning.trailing_zeros() as usize].category as usize)
            }
            _ => (0..9).rev().find(|&i| self.shown[i] > 0.0),
        };
        Bar {
            segments,
            edge,
            edge_strength: self.edge_strength,
            time: self.time,
        }
    }

    fn category_at(&self, x: Pixels) -> Option<Category> {
        let bounds = self.bar_bounds.get();
        let f = f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0);
        Category::ALL.into_iter().find(|&c| {
            let (start, end) = self.span(c);
            f >= start && f < end
        })
    }

    fn detail(&self, category: Category) -> Option<String> {
        let r = &self.report;
        let i = category as usize;
        let largest_file = || {
            r.largest_file[i].as_ref().map(|(name, bytes)| {
                format!("Your largest file is {name} at {}.", gb(*bytes as f64))
            })
        };
        match category {
            Category::Apps => r.largest_item[i].as_ref().map(|(name, bytes)| {
                let name = name.strip_suffix(".app").unwrap_or(name);
                format!("{name} is the largest at {}.", gb(*bytes as f64))
            }),
            Category::Developer => {
                let mut parts = Vec::new();
                if r.simulators >= 100_000_000 {
                    parts.push(format!("Simulators take {}.", gb(r.simulators as f64)));
                }
                if r.build_output >= 100_000_000 {
                    parts.push(format!("Build output takes {}.", gb(r.build_output as f64)));
                }
                if parts.is_empty() {
                    r.largest_item[i].as_ref().map(|(name, bytes)| {
                        format!("Most of it is {name} at {}.", gb(*bytes as f64))
                    })
                } else {
                    Some(parts.join(" "))
                }
            }
            Category::Photos | Category::Movies | Category::Documents | Category::Music => {
                largest_file()
            }
            Category::System => Some("macOS, app data and files outside your folders.".into()),
            Category::Caches => Some("Apps rebuild these when they need them again.".into()),
            Category::Trash => Some("Things you've already deleted.".into()),
        }
    }

    fn status_line(&self) -> AnyElement {
        let muted = rgb(MUTED);
        let path = |label: &'static str| {
            h_flex()
                .gap_2()
                .min_w_0()
                .child(div().flex_none().text_color(rgb(FG)).child(label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(tilde(&self.progress.current())),
                )
                .into_any_element()
        };
        match self.phase {
            Phase::Scanning => return path("Calculating"),
            Phase::Clearing => return path("Clearing"),
            Phase::Idle => {}
        }
        let available = gb(self.volume.total as f64 - self.used_shown());
        if let Some(c) = self.hover {
            return h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    Icon::new(icon(c))
                        .size_3p5()
                        .text_color(dither_gpui::hsla(tone(c).base())),
                )
                .child(div().flex_none().text_color(rgb(FG)).child(c.name()))
                .child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(gb(self.shown[c as usize])),
                )
                .children(
                    self.detail(c)
                        .map(|d| div().min_w_0().truncate().text_color(muted).child(d)),
                )
                .into_any_element();
        }
        if self.freed > 0 {
            return h_flex()
                .gap_1p5()
                .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                .child(
                    div()
                        .text_color(rgb(FG))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Freed {}.", gb(self.freed as f64))),
                )
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("{available} available")),
                )
                .into_any_element();
        }
        div()
            .text_color(muted)
            .child(format!("{available} available"))
            .into_any_element()
    }

    /// The line under the cleanup headline: the hovered item, what is being
    /// cleared, or what the disk looks like afterwards.
    fn clean_status(&self, items: &[Item]) -> AnyElement {
        let muted = rgb(MUTED);
        let item_line = |i: usize, text: String| {
            let item = &items[i];
            h_flex()
                .gap_2()
                .min_w_0()
                .children(
                    item.icon
                        .clone()
                        .map(|icon| img(icon).size(px(14.0)).flex_none()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(FG))
                        .child(item.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(gb(self.item_shown[i])),
                )
                .child(div().min_w_0().truncate().text_color(muted).child(text))
                .into_any_element()
        };
        let cleaning = self.progress.cleaning();
        if cleaning.count_ones() == 1 {
            return item_line(
                cleaning.trailing_zeros() as usize,
                tilde(&self.progress.current()),
            );
        }
        if cleaning != 0 {
            return h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(FG))
                        .child(format!("Clearing {} items", cleaning.count_ones())),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(muted)
                        .child(tilde(&self.progress.current())),
                )
                .into_any_element();
        }
        if let Some(i) = self.hover_item {
            return item_line(i, items[i].detail.clone());
        }
        let free = self.volume.total as f64 - self.used_shown();
        div()
            .text_color(muted)
            .child(format!(
                "{} available after clearing",
                gb(free + self.selected_bytes())
            ))
            .into_any_element()
    }

    /// The treemap: tiles painted on one canvas, with an invisible div per
    /// tile on top for hover, clicks and the label.
    fn treemap(&self, items: &[Item], cx: &mut Context<Self>) -> impl IntoElement + use<> {
        const DUST: f32 = 60.0;
        let tiles = self.tiles();
        let cleaning = self.progress.cleaning();
        let weights = self.weights();
        let inset = |r: Rect| Rect {
            x: r.x + treemap::GAP / 2.0,
            y: r.y + treemap::GAP / 2.0,
            w: (r.w - treemap::GAP).max(0.0),
            h: (r.h - treemap::GAP).max(0.0),
        };
        let painted: Vec<(Rect, treemap::Tile)> = items
            .iter()
            .enumerate()
            .filter(|&(i, _)| tiles[i].w >= treemap::GAP + 2.0 && tiles[i].h >= treemap::GAP + 2.0)
            .map(|(i, item)| {
                let tile = treemap::Tile {
                    tone: item.tone,
                    on: self.selected[i],
                    glow: self.item_glow[i],
                    reveal: (self.item_shown[i] / weights[i].max(1.0)) as f32,
                    busy: busy(cleaning, i),
                };
                (inset(tiles[i]), tile)
            })
            .collect();

        let idle = self.phase == Phase::Idle;
        let mut labels = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let r = inset(tiles[i]);
            if r.w < 2.0 || r.h < 2.0 {
                continue;
            }
            let on = self.selected[i];
            let text = rgb(if on { FG } else { 0x77777c });
            let small = r.w < 96.0 || r.h < 56.0;
            let tiny = r.w < 34.0 || r.h < 30.0;
            let icon = item.icon.clone().map(|icon| {
                img(icon)
                    .size(px(if small { 14.0 } else { 17.0 }))
                    .flex_none()
                    .grayscale(!on)
                    .opacity(if on { 1.0 } else { 0.5 })
            });
            let label =
                v_flex()
                    .size_full()
                    .justify_between()
                    .p(px(if small { 6.0 } else { 10.0 }))
                    .overflow_hidden()
                    .when(!tiny, |label| {
                        label
                            .child(h_flex().gap(px(7.0)).min_w_0().children(icon).when(
                                !small,
                                |row| {
                                    row.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(text)
                                            .child(item.name.clone()),
                                    )
                                },
                            ))
                            .when(r.h >= 44.0 && r.w >= 58.0, |label| {
                                label.child(
                                    div()
                                        .whitespace_nowrap()
                                        .font_family("Menlo")
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(px(if small { 10.5 } else { 14.0 }))
                                        .text_color(text)
                                        .child(gb(self.item_shown[i])),
                                )
                            })
                    });
            labels.push(
                div()
                    .id(("tile", i))
                    .absolute()
                    .left(px(r.x))
                    .top(px(r.y))
                    .w(px(r.w))
                    .h(px(r.h))
                    .when(idle, |tile| tile.cursor_pointer())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if this.phase != Phase::Idle || this.autoplay {
                            return;
                        }
                        if *hovered {
                            this.hover_item = Some(i);
                        } else if this.hover_item == Some(i) {
                            this.hover_item = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.phase == Phase::Idle {
                            this.selected[i] = !this.selected[i];
                            cx.notify();
                        }
                    }))
                    .child(label),
            );
        }

        let clean_bounds = self.clean_bounds.clone();
        let particles = self.particles.clone();
        let time = self.time;
        div()
            .relative()
            .flex_1()
            .min_h(px(160.0))
            .w_full()
            .child(
                canvas(
                    move |bounds, _, _| clean_bounds.set(bounds),
                    move |bounds, _, window, _| {
                        for (r, tile) in &painted {
                            let at = Bounds::new(
                                bounds.origin + point(px(r.x), px(r.y)),
                                size(px(r.w), px(r.h)),
                            );
                            treemap::paint(tile, at, bounds.origin, time, window);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(labels)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let origin = bounds.origin + point(px(0.0), px(DUST));
                        bar::paint_particles(&particles, origin, window);
                    },
                )
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .top(px(-DUST)),
            )
    }

    fn cleanup(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = h_flex().h(px(30.0)).gap_2().items_center();
        let Some(items) = self.items.clone() else {
            return row
                .child(Spinner::new().small().color(rgb(MUTED).into()))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Looking for things you can clear"),
                )
                .into_any_element();
        };
        let visible = self.visible();
        if visible.is_empty() {
            return row
                .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Nothing else is worth clearing right now."),
                )
                .into_any_element();
        }

        let selected = self.selected_bytes();
        let idle = self.phase == Phase::Idle;
        let header = h_flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(gb(selected)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(rgb(MUTED))
                                    .child("can go"),
                            ),
                    )
                    .child(div().h(px(18.0)).child(self.clean_status(&items))),
            )
            .child(
                Button::new("clear")
                    .primary()
                    .small()
                    .label("Clear")
                    .disabled(!idle || selected < 1e6)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear();
                        cx.notify();
                    })),
            );

        let map = self.treemap(&items, cx);
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(header)
            .child(map)
            .into_any_element()
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.animating = self.tick();
        if self.animating {
            window.request_animation_frame();
        }
        let idle = self.phase == Phase::Idle;

        let header = h_flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_size(px(26.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(gb(self.used_shown())),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(MUTED))
                                    .child(format!("of {} used", capacity(self.volume.total))),
                            ),
                    )
                    .child(div().h(px(18.0)).child(self.status_line())),
            )
            .child(
                Button::new("rescan")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("Rescan")
                    .disabled(!idle)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scan();
                        cx.notify();
                    })),
            );

        let model = self.bar();
        let bar_bounds = self.bar_bounds.clone();
        let bar = div()
            .id("bar")
            .relative()
            .h(px(54.0))
            .w_full()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.phase != Phase::Idle
                    || this.autoplay
                    || !this.bar_bounds.get().contains(&e.position)
                {
                    return;
                }
                let hover = this.category_at(e.position.x);
                if hover != this.hover {
                    this.hover = hover;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && !this.autoplay {
                    this.hover = None;
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| bar_bounds.set(bounds),
                    move |bounds, _, window, _| bar::paint(&model, bounds, window),
                )
                .absolute()
                .size_full(),
            );

        let legend_bounds = self.legend_bounds.clone();
        let [pill_x, pill_y, pill_w] = self.pill;
        let pill = div()
            .absolute()
            .left(px(pill_x))
            .top(px(pill_y))
            .w(px(pill_w))
            .h(px(ROW_H))
            .rounded(px(6.0))
            .bg(rgb(0x232327))
            .opacity(self.pill_alpha);
        let rows = Category::ALL.chunks(3).map(|row| {
            h_flex().gap(px(COL_GAP)).children(row.iter().map(|&c| {
                h_flex()
                    .id(c.name())
                    .flex_1()
                    .h(px(ROW_H))
                    .px_1p5()
                    .gap_2()
                    .rounded(px(6.0))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if this.phase != Phase::Idle || this.autoplay {
                            return;
                        }
                        if *hovered {
                            this.hover = Some(c);
                        } else if this.hover == Some(c) {
                            this.hover = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(icon(c))
                            .size_3p5()
                            .text_color(dither_gpui::hsla(tone(c).base())),
                    )
                    .child(div().flex_1().text_color(rgb(FG)).child(c.name()))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(MUTED))
                            .child(gb(self.shown[c as usize])),
                    )
            }))
        });
        let legend = v_flex()
            .relative()
            .gap(px(ROW_GAP))
            .child(pill)
            .child(
                canvas(
                    move |bounds, _, _| legend_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rows);

        v_flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .text_size(px(13.0))
            .px_5()
            .pt_5()
            .pb_3()
            .gap_4()
            .child(header)
            .child(bar)
            .child(legend)
            .child(div().flex_none().h(px(1.0)).bg(rgb(LINE)))
            .child(self.cleanup(cx))
    }
}

/// Whether item `i` is set in a [`Progress::cleaning`] mask.
fn busy(mask: u64, i: usize) -> bool {
    1u64.checked_shl(i as u32)
        .is_some_and(|bit| mask & bit != 0)
}
