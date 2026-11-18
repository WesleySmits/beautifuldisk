//! "Your files": copies of the same file and old screenshots, as a mosaic of
//! dithered pictures in place of the cache treemap. Clicking a tile opens
//! its copies side by side, where the one that stays shows its picture and
//! the spares crumble into dots. Nothing is picked until the user picks it,
//! and picked files go to the Trash, so they can be put back.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use dither_gpui::dither_core::Tone;
use dither_gpui::dither_core::dither::hash01;
use dither_gpui::dither_core::ease::approach;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::docker_panel::size as bytes_text;
use super::{BG, Card, FG, MUTED, OK, Phase, WARN, tilde, tip};
use crate::bar::{self, Particle};
use crate::files::{self, Entry, Found, Kind, SHOWN, Search};
use crate::mosaic;
use crate::treemap::{self, Rect};

/// How long the band of light takes to cross while files go to the Trash.
const SWEEP: f32 = 1.1;
/// How long a tile takes to crumble.
const CRUMBLE: f32 = 0.6;
/// Copies shown side by side when a tile is open.
const SIDE_BY_SIDE: usize = 3;

/// Each moved file and where it went in the Trash.
type Moved = Vec<(PathBuf, PathBuf)>;

#[derive(Default)]
struct Trashing {
    /// Entries done, with where each file went.
    done: Mutex<Vec<(usize, Moved)>>,
    finished: AtomicBool,
}

#[derive(Default)]
pub(super) struct FilesView {
    search: Option<Arc<Search>>,
    found: Option<Arc<Found>>,
    picked: Vec<bool>,
    /// Per entry, which copy stays.
    keep: Vec<usize>,
    /// Per entry, when it crumbled (moved to the Trash).
    gone_at: Vec<Option<f32>>,
    /// Per tile: glow, how far its picture has developed, and its area.
    glow: Vec<f32>,
    develop: Vec<f32>,
    weight: Vec<f64>,
    hover: Option<usize>,
    /// The entry open side by side, and how far it has opened.
    open: Option<usize>,
    open_at: f32,
    /// In the open entry: how much each copy shows its picture, and how far
    /// the spares have crumbled.
    copy_develop: Vec<f32>,
    spare_dissolve: f32,
    /// When the tiles arrived.
    at: f32,
    trashing: Option<Arc<Trashing>>,
    /// Where the band of light started, while files go to the Trash.
    sweep_at: Option<f32>,
    /// Entries done but not yet reached by the band.
    pending: Vec<usize>,
    moved: Vec<(PathBuf, PathBuf)>,
    moved_entries: Vec<usize>,
    moved_bytes: u64,
    particles: Vec<Particle>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// `--autoplay`: when the tour of this tab began, and its next step.
    tour_at: Option<f32>,
    tour_step: usize,
}

impl FilesView {
    fn tiles_len(&self) -> usize {
        self.found.as_ref().map_or(0, |f| {
            f.entries.len().min(SHOWN) + usize::from(f.entries.len() > SHOWN)
        })
    }

    /// Entries a tile stands for: one, or every entry past [`SHOWN`].
    fn members(&self, tile: usize) -> std::ops::Range<usize> {
        let n = self.found.as_ref().map_or(0, |f| f.entries.len());
        if tile < SHOWN {
            tile..tile + 1
        } else {
            SHOWN..n
        }
    }

    fn bytes_left(&self) -> u64 {
        let Some(found) = &self.found else { return 0 };
        found
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| self.gone_at[*i].is_none() && !self.pending.contains(i))
            .map(|(_, e)| e.bytes)
            .sum()
    }

    fn picked_bytes(&self) -> u64 {
        let Some(found) = &self.found else { return 0 };
        found
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| self.picked[*i] && self.gone_at[*i].is_none())
            .map(|(_, e)| e.bytes)
            .sum()
    }

    fn busy(&self) -> bool {
        self.trashing.is_some() || self.sweep_at.is_some()
    }
}

fn months_old(modified: SystemTime) -> String {
    let days = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    match days / 30 {
        m @ 0..=11 => format!("{} months old", m.max(1)),
        m if m < 24 => "1 year old".into(),
        m => format!("{} years old", m / 12),
    }
}

fn check(on: bool) -> Div {
    div()
        .size(px(15.0))
        .flex_none()
        .rounded(px(4.0))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .when(on, |c| {
            c.bg(rgb(FG)).border_color(rgb(FG)).child(
                svg()
                    .path(IconName::Check.path())
                    .size(px(11.0))
                    .text_color(rgb(BG)),
            )
        })
        .when(!on, |c| c.border_color(rgb(0x44444a)))
}

fn tone(entry: &Entry) -> Tone {
    match entry.kind {
        Kind::Copies(_) => Tone::Purple,
        Kind::Screenshot(_) => Tone::Orange,
    }
}

impl Card {
    /// Starts looking once the scan's own results are in.
    pub(super) fn start_files(&mut self) {
        let search = Arc::new(Search::default());
        if self.demo {
            files::demo(search.clone());
        } else {
            files::find(search.clone());
        }
        self.files = FilesView {
            search: Some(search),
            ..Default::default()
        };
    }

    /// Eases the mosaic and takes in the worker's results. Returns whether
    /// another frame is needed.
    pub(super) fn files_tick(&mut self, dt: f32) -> bool {
        let time = self.time;
        let f = &mut self.files;
        let Some(search) = &f.search else {
            return false;
        };
        if f.found.is_none() {
            let Some(found) = search.found() else {
                return true;
            };
            let n = found.entries.len();
            f.picked = vec![false; n];
            f.keep = vec![0; n];
            f.gone_at = vec![None; n];
            f.found = Some(found);
            let tiles = f.tiles_len();
            f.glow = vec![0.0; tiles];
            f.develop = vec![0.0; tiles];
            f.weight = (0..tiles).map(|t| tile_weight(f, t)).collect();
            f.at = time;
        }
        let mut moving = false;
        // Files done move once the band of light reaches their tile.
        if let Some(trashing) = &f.trashing {
            for (i, moved) in trashing.done.lock().unwrap().drain(..) {
                if moved.is_empty() {
                    continue;
                }
                f.moved.extend(moved);
                f.moved_entries.push(i);
                f.pending.push(i);
            }
            if trashing.finished.load(Relaxed) && f.pending.is_empty() {
                f.trashing = None;
            }
        }
        if let Some(start) = f.sweep_at {
            let area = f.bounds.get().size;
            let band = band_x(time - start, f32::from(area.width));
            let tiles = layout(f, area);
            let mut whoosh = false;
            f.pending.retain(|&i| {
                let tile = i.min(SHOWN);
                let reached =
                    tiles.get(tile).is_none_or(|r| band >= r.x + r.w / 2.0) || time - start > SWEEP;
                if reached {
                    f.gone_at[i] = Some(time);
                    f.moved_bytes += f.found.as_ref().map_or(0, |found| found.entries[i].bytes);
                    whoosh = true;
                }
                !reached
            });
            if whoosh {
                self.sound.whoosh();
            }
            let f = &mut self.files;
            if time - start > SWEEP && f.trashing.is_none() && f.pending.is_empty() {
                f.sweep_at = None;
            }
            moving = true;
        }
        let f = &mut self.files;

        // Dust off every tile crumbling.
        let area = f.bounds.get().size;
        let rects = layout(f, area);
        let mut emit = Vec::new();
        for (t, r) in rects.iter().enumerate() {
            let crumbling = f
                .members(t)
                .any(|i| f.gone_at[i].is_some_and(|at| time - at < CRUMBLE));
            if crumbling && r.w > 2.0 && r.h > 2.0 {
                let e = &f.found.as_ref().unwrap().entries[f.members(t).start];
                emit.push(([r.x, r.y, r.x + r.w, r.y + r.h], tone(e)));
            }
        }
        let rate = 220.0 / (emit.len().max(1) as f32).sqrt();
        bar::step_particles(&mut f.particles, &mut self.rng, dt, &emit, rate);
        moving |= !f.particles.is_empty();

        for t in 0..f.glow.len() {
            let members = f.members(t);
            let on = members.clone().any(|i| f.picked[i]);
            let hovered = f.hover == Some(t);
            let glow_to = match (hovered, on) {
                (true, _) => 1.0,
                (false, true) => 0.62,
                (false, false) => 0.25,
            };
            moving |= ease(&mut f.glow[t], glow_to, dt, 0.06);
            let develop_to = if hovered { 1.0 } else { 0.0 };
            moving |= ease(&mut f.develop[t], develop_to, dt, 0.08);
            // A tile gives its room to its neighbours once it has crumbled.
            let gone = members
                .clone()
                .all(|i| f.gone_at[i].is_some_and(|at| time - at > CRUMBLE));
            let to = if gone { 0.0 } else { tile_weight(f, t) };
            let w = approach(f.weight[t] as f32, to as f32, dt, 0.08) as f64;
            f.weight[t] = if (w - to).abs() < 1e-3 { to } else { w };
            moving |= f.weight[t] != to;
        }
        if time - f.at < 1.0 {
            moving = true;
        }

        let open_to = if f.open.is_some() { 1.0 } else { 0.0 };
        moving |= ease(&mut f.open_at, open_to, dt, 0.06);
        if let Some(i) = f.open {
            let keep = f.keep[i];
            let picked = f.picked[i];
            for (k, d) in f.copy_develop.iter_mut().enumerate() {
                let to = if k == keep { 1.0 } else { 0.0 };
                moving |= ease(d, to, dt, 0.08);
            }
            let to = if picked { 0.7 } else { 0.0 };
            moving |= ease(&mut f.spare_dissolve, to, dt, 0.08);
        }
        moving
    }

    /// `--autoplay`, once the caches are cleared: this tab, one file's
    /// copies side by side, the other copy kept instead, then everything
    /// picked and moved to the Trash. Returns whether it is still playing.
    pub(super) fn play_files(&mut self) -> bool {
        const STEPS: [f32; 8] = [0.8, 2.0, 2.8, 4.6, 5.8, 7.0, 7.6, 8.6];
        if self.files.found.is_none() {
            return true;
        }
        let start = *self.files.tour_at.get_or_insert(self.time);
        let t = self.time - start;
        let step = self.files.tour_step;
        let Some(&at) = STEPS.get(step) else {
            return t < STEPS[STEPS.len() - 1] + 3.5;
        };
        if t < at {
            return true;
        }
        self.files.tour_step += 1;
        let f = &mut self.files;
        match step {
            0 => self.view = super::View::Files,
            1 => f.hover = Some(0),
            2 => self.open_entry(0),
            3 => f.keep[0] = 1,
            4 => f.picked[0] = true,
            5 => f.open = None,
            6 => f.picked.fill(true),
            _ => self.trash_picked(),
        }
        true
    }

    pub(super) fn files_dismiss(&mut self) -> bool {
        if self.files.open.take().is_some() {
            return true;
        }
        false
    }

    fn open_entry(&mut self, i: usize) {
        let f = &mut self.files;
        let Some(found) = &f.found else { return };
        f.open = Some(i);
        f.hover = None;
        f.copy_develop = vec![0.0; found.entries[i].files().len()];
        f.spare_dissolve = if f.picked[i] { 0.7 } else { 0.0 };
    }

    fn trash_picked(&mut self) {
        let f = &mut self.files;
        let Some(found) = f.found.clone() else { return };
        let jobs: Vec<(usize, Vec<PathBuf>)> = (0..found.entries.len())
            .filter(|&i| f.picked[i] && f.gone_at[i].is_none())
            .map(|i| {
                let paths = match &found.entries[i].kind {
                    Kind::Copies(copies) => copies
                        .iter()
                        .enumerate()
                        .filter(|&(k, _)| k != f.keep[i])
                        .map(|(_, c)| c.path.clone())
                        .collect(),
                    Kind::Screenshot(file) => vec![file.path.clone()],
                };
                (i, paths)
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        let trashing = Arc::new(Trashing::default());
        f.trashing = Some(trashing.clone());
        f.sweep_at = Some(self.time);
        f.open = None;
        f.hover = None;
        f.moved.clear();
        f.moved_entries.clear();
        f.moved_bytes = 0;
        let demo = self.demo;
        thread::spawn(move || {
            for (i, paths) in jobs {
                let moved = if demo {
                    paths.iter().map(|p| (p.clone(), p.clone())).collect()
                } else {
                    files::trash(paths)
                };
                trashing.done.lock().unwrap().push((i, moved));
            }
            trashing.finished.store(true, Relaxed);
        });
    }

    fn put_back(&mut self) {
        let f = &mut self.files;
        if !self.demo {
            files::put_back(&f.moved);
        }
        for &i in &f.moved_entries {
            f.gone_at[i] = None;
            f.picked[i] = false;
        }
        f.moved.clear();
        f.moved_entries.clear();
        f.moved_bytes = 0;
        f.at = self.time;
    }

    /// The headline figure for the "Your files" tab.
    pub(super) fn files_figure(&self) -> String {
        let f = &self.files;
        let bytes = match (&f.found, &f.search) {
            (Some(_), _) => f.bytes_left(),
            (None, Some(search)) => search.copies.load(Relaxed),
            _ => 0,
        };
        // A few MB of copies is common, and "0.0 GB" would hide them.
        if bytes == 0 {
            super::gb(0.0)
        } else {
            bytes_text(bytes)
        }
    }

    pub(super) fn files_status(&self) -> AnyElement {
        let f = &self.files;
        let muted = rgb(MUTED);
        let Some(found) = &f.found else {
            let current = f.search.as_ref().map(|s| s.current()).unwrap_or_default();
            return h_flex()
                .h_full()
                .min_w_0()
                .items_center()
                .child(crate::pixel_text::status(
                    "Comparing",
                    tilde(&current),
                    self.time,
                    MUTED,
                ))
                .into_any_element();
        };
        if f.busy() {
            return div()
                .text_color(rgb(FG))
                .child("Moving to the Trash")
                .into_any_element();
        }
        if f.moved_bytes > 0 {
            return h_flex()
                .gap_1p5()
                .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                .child(
                    div()
                        .text_color(rgb(FG))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Moved {} to the Trash.", bytes_text(f.moved_bytes))),
                )
                .child(div().text_color(muted).child("Empty it to free the space"))
                .into_any_element();
        }
        let picked = f.picked_bytes();
        if picked > 0 {
            return div()
                .text_color(muted)
                .child(format!("{} picked", bytes_text(picked)))
                .into_any_element();
        }
        let copies = found
            .entries
            .iter()
            .filter(|e| matches!(e.kind, Kind::Copies(_)))
            .count();
        let shots = found.entries.len() - copies;
        let mut parts = Vec::new();
        if copies > 0 {
            parts.push(count(copies, "file with copies", "files with copies"));
        }
        if shots > 0 {
            parts.push(count(shots, "old screenshot", "old screenshots"));
        }
        let text = if parts.is_empty() {
            "No copies or old screenshots in your folders".to_string()
        } else {
            format!("{}. Click one to look", parts.join(", "))
        };
        div()
            .min_w_0()
            .truncate()
            .text_color(muted)
            .child(text)
            .into_any_element()
    }

    pub(super) fn files_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.files;
        let idle = self.phase == Phase::Idle && !f.busy();
        let any_left = f
            .found
            .as_ref()
            .is_some_and(|found| (0..found.entries.len()).any(|i| f.gone_at[i].is_none()));
        let all_picked = f.found.as_ref().is_some_and(|found| {
            (0..found.entries.len()).all(|i| f.picked[i] || f.gone_at[i].is_some())
        });
        h_flex()
            .gap_1()
            .when(!f.moved.is_empty() && idle, |row| {
                row.child(
                    Button::new("put-back")
                        .ghost()
                        .small()
                        .label("Put back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.put_back();
                            cx.notify();
                        })),
                )
            })
            .when(any_left, |row| {
                row.child(
                    Button::new("pick-all")
                        .ghost()
                        .small()
                        .label(if all_picked { "Pick none" } else { "Pick all" })
                        .disabled(!idle)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let f = &mut this.files;
                            for i in 0..f.picked.len() {
                                f.picked[i] = !all_picked && f.gone_at[i].is_none();
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("trash")
                    .primary()
                    .small()
                    .label("Move to Trash")
                    .disabled(!idle || f.picked_bytes() == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.trash_picked();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The area under the headline: the search, the mosaic, or one entry
    /// open side by side.
    pub(super) fn files_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.files;
        let bounds = f.bounds.clone();
        let frame = div().relative().flex_1().min_h(px(160.0)).w_full().child(
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        );
        let Some(found) = f.found.clone() else {
            let search = f.search.clone();
            let time = self.time;
            return frame
                .child(
                    canvas(
                        |_, _, _| {},
                        move |b, _, window, _| paint_search(search.as_deref(), b, time, window),
                    )
                    .absolute()
                    .size_full(),
                )
                .into_any_element();
        };
        if found.entries.is_empty() {
            return frame
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Check).size_3p5().text_color(rgb(OK)))
                        .child(
                            div()
                                .text_color(rgb(MUTED))
                                .child("Nothing here worth a look right now."),
                        ),
                )
                .into_any_element();
        }
        if let Some(i) = f.open {
            return frame
                .child(self.open_view(&found, i, cx))
                .into_any_element();
        }
        frame.child(self.mosaic(&found, cx)).into_any_element()
    }

    fn mosaic(&self, found: &Arc<Found>, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.files;
        let time = self.time;
        let rects = layout(f, f.bounds.get().size);
        let band = f
            .sweep_at
            .map(|start| band_x(time - start, f32::from(f.bounds.get().size.width)));
        let idle = self.phase == Phase::Idle && !f.busy();
        let mut tiles = Vec::new();
        let mut labels = Vec::new();
        for (t, r) in rects.iter().enumerate() {
            let r = inset(*r);
            if r.w < 2.0 || r.h < 2.0 {
                continue;
            }
            let members = f.members(t);
            let first = &found.entries[members.start];
            let on = members.clone().any(|i| f.picked[i]);
            let dissolve = members
                .clone()
                .filter_map(|i| f.gone_at[i])
                .map(|at| ((time - at) / CRUMBLE).clamp(0.0, 1.0))
                .fold(0.0f32, f32::max);
            let more = t >= SHOWN;
            let thumb = (!more)
                .then(|| {
                    found
                        .thumbs
                        .get(t)
                        .and_then(|s| s.get())
                        .and_then(|o| o.as_ref())
                })
                .flatten()
                .map(|_| t);
            let reveal = (((time - f.at) - t as f32 * 0.03) / 0.5).clamp(0.0, 1.0);
            tiles.push((
                r,
                thumb,
                mosaic::Tile {
                    tone: if more { Tone::Grey } else { tone(first) },
                    on,
                    glow: f.glow[t],
                    develop: f.develop[t],
                    dissolve,
                    reveal,
                    band,
                },
            ));
            if dissolve > 0.0 {
                continue;
            }
            let label = if more {
                Some(format!("{} more", members.len()))
            } else if r.w >= 70.0 && r.h >= 40.0 {
                Some(bytes_text(first.bytes))
            } else {
                None
            };
            let entries: Vec<usize> = members.clone().collect();
            let found2 = found.clone();
            let keep = f.keep.clone();
            let picked = f.picked.clone();
            labels.push(
                div()
                    .id(("file-tile", t))
                    .absolute()
                    .left(px(r.x))
                    .top(px(r.y))
                    .w(px(r.w))
                    .h(px(r.h))
                    .p(px(6.0))
                    .flex()
                    .flex_col()
                    .justify_end()
                    .items_start()
                    .overflow_hidden()
                    .when(idle, |tile| tile.cursor_pointer())
                    .when(idle, |tile| {
                        tile.tooltip(move |_, cx| {
                            tip::Tip::files(&found2, &entries, &keep, &picked).view(cx)
                        })
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let f = &mut this.files;
                        if *hovered {
                            f.hover = Some(t);
                        } else if f.hover == Some(t) {
                            f.hover = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.phase != Phase::Idle || this.files.busy() {
                            return;
                        }
                        if t >= SHOWN {
                            // Too many to look at one by one: picks them all.
                            let f = &mut this.files;
                            let on = !f.members(t).any(|i| f.picked[i]);
                            for i in f.members(t) {
                                f.picked[i] = on && f.gone_at[i].is_none();
                            }
                        } else {
                            this.open_entry(t);
                        }
                        cx.notify();
                    }))
                    .children(label.map(|label| {
                        div()
                            .px_1()
                            .rounded(px(4.0))
                            .bg(rgba(BG << 8 | 0xb0))
                            .whitespace_nowrap()
                            .font_family("Menlo")
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(11.0))
                            .text_color(rgb(FG))
                            .child(label)
                    })),
            );
        }
        let particles = f.particles.clone();
        let found = found.clone();
        div()
            .absolute()
            .size_full()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        for (r, thumb, tile) in &tiles {
                            let at = Bounds::new(
                                bounds.origin + point(px(r.x), px(r.y)),
                                size(px(r.w), px(r.h)),
                            );
                            let thumb = thumb
                                .and_then(|t| found.thumbs[t].get())
                                .and_then(|t| t.as_ref());
                            mosaic::paint(tile, thumb, at, bounds.origin, time, window);
                        }
                        bar::paint_particles(&particles, bounds.origin, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(labels)
            .into_any_element()
    }

    /// One entry, opened: its copies side by side. The copy that stays shows
    /// its picture; click another to keep that one instead.
    fn open_view(&self, found: &Arc<Found>, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.files;
        let entry = &found.entries[i];
        let picked = f.picked[i];
        let keep = f.keep[i];
        let time = self.time;
        let open = f.open_at;
        let thumb_slot = i;
        let copies = entry.files();
        let shown = copies.len().min(SIDE_BY_SIDE);
        let is_copies = matches!(entry.kind, Kind::Copies(_));

        let columns = (0..shown).map(|k| {
            let file = &copies[k];
            let kept = is_copies && k == keep;
            let develop = if is_copies {
                f.copy_develop.get(k).copied().unwrap_or(0.0)
            } else {
                1.0
            } * open;
            let tile = mosaic::Tile {
                tone: tone(entry),
                on: picked && !kept,
                glow: 0.6,
                develop,
                dissolve: if kept || !is_copies {
                    0.0
                } else {
                    f.spare_dissolve
                },
                reveal: open,
                band: None,
            };
            let found = found.clone();
            let (status, color) = match (is_copies, kept, picked) {
                (true, true, _) => ("Kept", OK),
                (_, false, true) | (false, _, true) => ("To Trash", WARN),
                (true, false, false) => ("Spare copy", MUTED),
                (false, _, false) => ("Stays", MUTED),
            };
            let folder = file
                .path
                .parent()
                .map(|p| tilde(&p.to_string_lossy()))
                .unwrap_or_default();
            v_flex()
                .id(("copy", k))
                .flex_1()
                .min_w_0()
                .gap_1()
                .when(is_copies && !kept, |c| {
                    c.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.files.keep[i] = k;
                            cx.notify();
                        }))
                })
                .child(
                    div().relative().flex_1().min_h_0().child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let thumb = found
                                    .thumbs
                                    .get(thumb_slot)
                                    .and_then(|s| s.get())
                                    .and_then(|t| t.as_ref());
                                mosaic::paint(&tile, thumb, bounds, bounds.origin, time, window);
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
                )
                .child(
                    div()
                        .pt_1()
                        .truncate()
                        .text_color(rgb(FG))
                        .child(files::file_name(&file.path)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_size(px(12.0))
                        .child(div().flex_none().text_color(rgb(color)).child(status))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .text_color(rgb(MUTED))
                                .child(if is_copies {
                                    folder
                                } else {
                                    months_old(file.modified)
                                }),
                        ),
                )
        });

        let pick_label = match (&entry.kind, copies.len()) {
            (Kind::Copies(_), 2) => "Move the spare copy to the Trash",
            (Kind::Copies(_), _) => "Move the spare copies to the Trash",
            (Kind::Screenshot(_), _) => "Move it to the Trash",
        };
        let idle = !f.busy();
        v_flex()
            .absolute()
            .size_full()
            .gap_3()
            .opacity(open)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .child(entry.name()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("Menlo")
                            .font_weight(FontWeight::BOLD)
                            .child(bytes_text(entry.bytes)),
                    )
                    .when(copies.len() > shown, |row| {
                        row.child(
                            div()
                                .text_color(rgb(MUTED))
                                .child(format!("and {} more copies", copies.len() - shown)),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("close-copies")
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .label("Back")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files.open = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap_3()
                    .children(columns),
            )
            .child(
                h_flex()
                    .id("pick-entry")
                    .gap_2p5()
                    .when(idle, |row| {
                        row.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files.picked[i] = !this.files.picked[i];
                                cx.notify();
                            }))
                    })
                    .child(check(picked))
                    .child(div().child(pick_label)),
            )
            .into_any_element()
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn ease(value: &mut f32, to: f32, dt: f32, time: f32) -> bool {
    *value = approach(*value, to, dt, time);
    if (*value - to).abs() < 0.004 {
        *value = to;
        false
    } else {
        true
    }
}

/// Tiles by the square root of their bytes, so a few big files don't leave
/// every screenshot a sliver.
fn tile_weight(f: &FilesView, tile: usize) -> f64 {
    let Some(found) = &f.found else { return 0.0 };
    let bytes: u64 = f.members(tile).map(|i| found.entries[i].bytes).sum();
    (bytes as f64).sqrt()
}

fn layout(f: &FilesView, area: Size<Pixels>) -> Vec<Rect> {
    let initial: Vec<f64> = (0..f.weight.len()).map(|t| tile_weight(f, t)).collect();
    let area = Rect {
        x: 0.0,
        y: 0.0,
        w: f32::from(area.width),
        h: f32::from(area.height),
    };
    treemap::layout(&initial, &f.weight, area)
}

fn inset(r: Rect) -> Rect {
    Rect {
        x: r.x + treemap::GAP / 2.0,
        y: r.y + treemap::GAP / 2.0,
        w: (r.w - treemap::GAP).max(0.0),
        h: (r.h - treemap::GAP).max(0.0),
    }
}

/// Where the band of light is, `t` seconds into its sweep across `width`.
fn band_x(t: f32, width: f32) -> f32 {
    -60.0 + (width + 120.0) * (t / SWEEP)
}

/// While comparing: every file is a dot, and a front of light crosses as
/// they are checked. Dots behind it with a twin turn purple.
fn paint_search(search: Option<&Search>, bounds: Bounds<Pixels>, time: f32, window: &mut Window) {
    use dither_gpui::dither_core::Rgba;
    const PITCH: f32 = 8.0;
    let Some(search) = search else { return };
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let cols = (w / PITCH) as i32;
    let rows = (h / PITCH) as i32;
    let total = search.total.load(Relaxed).max(1) as f32;
    let checked = search.checked.load(Relaxed) as f32;
    let share = if search.total.load(Relaxed) == 0 {
        // Still listing folders: the front idles at the start.
        0.0
    } else {
        (checked / total).min(1.0)
    };
    let front = share * (cols + 6) as f32;
    let twins = search.copies.load(Relaxed) > 0;
    let purple = Tone::Purple.base();
    for row in 0..rows {
        for col in 0..cols {
            let h = hash01(col, row);
            if h < 0.35 {
                continue;
            }
            let checked = (col as f32) < front - hash01(row, col) * 4.0;
            let ahead = (col as f32 - front).abs() < 2.5;
            let (color, dot) = if checked && twins && h > 0.94 {
                if front - (col as f32) < 3.0 {
                    (Rgba::WHITE, 4.0)
                } else {
                    (purple, 3.0)
                }
            } else if checked {
                (Rgba::hex(0x2a2a2e), 3.0)
            } else if ahead {
                (Rgba::hex(0x8c8c92), 3.0)
            } else {
                // Not reached yet: a slow shimmer while it waits.
                let a = 0.5 + 0.2 * ((time * 2.0 + (col + row) as f32 * 0.3).sin());
                (Rgba::hex(0x3a3a40).with_alpha((a * 255.0) as u8), 3.0)
            };
            let at = bounds.origin
                + point(
                    px(col as f32 * PITCH + (4.0 - dot) / 2.0),
                    px(row as f32 * PITCH + (4.0 - dot) / 2.0),
                );
            window.paint_quad(fill(
                Bounds::new(at, size(px(dot), px(dot))),
                dither_gpui::hsla(color),
            ));
        }
    }
}

impl tip::Tip {
    /// For a tile: which copy stays and which go, or the screenshots.
    pub(super) fn files(found: &Found, entries: &[usize], keep: &[usize], picked: &[bool]) -> Self {
        let first = &found.entries[entries[0]];
        if entries.len() > 1 {
            let rows = entries
                .iter()
                .map(|&i| {
                    let e = &found.entries[i];
                    (e.name(), Some(bytes_text(e.bytes)), false)
                })
                .collect();
            let bytes = entries.iter().map(|&i| found.entries[i].bytes).sum();
            return tip::Tip::rows(format!("{} more", entries.len()), bytes_text(bytes), rows);
        }
        let i = entries[0];
        let rows = match &first.kind {
            Kind::Copies(copies) => copies
                .iter()
                .enumerate()
                .map(|(k, c)| {
                    let how = if k == keep[i] {
                        "Kept"
                    } else if picked[i] {
                        "To Trash"
                    } else {
                        "Spare copy"
                    };
                    (
                        tilde(&c.path.to_string_lossy()),
                        Some(how.to_string()),
                        k == keep[i],
                    )
                })
                .collect(),
            Kind::Screenshot(file) => vec![(
                tilde(&file.path.to_string_lossy()),
                Some(months_old(file.modified)),
                false,
            )],
        };
        tip::Tip::rows(first.name(), bytes_text(first.bytes), rows)
    }
}
