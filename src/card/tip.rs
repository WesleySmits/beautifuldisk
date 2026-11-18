//! The hover card for treemap tiles and Docker's old builds: what clearing
//! removes, and what stays.

use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::docker_panel::{count, size};
use super::{FG, LINE, MUTED, OK, gb, tilde};
use crate::clutter::Item;
use crate::docker::Builds;
use crate::ui::z;

/// Rows shown before the rest are counted.
const ROWS: usize = 6;

#[derive(Clone)]
pub(super) struct Tip {
    icon: Option<Arc<Image>>,
    title: String,
    size: String,
    /// What happens to every row, when it is the same for all.
    note: Option<String>,
    rows: Vec<Row>,
    more: usize,
}

#[derive(Clone)]
struct Row {
    /// Path or name.
    what: String,
    /// What happens to it, or its size.
    how: Option<String>,
    /// Stays, so it reads green.
    kept: bool,
}

impl Row {
    fn new(what: String, how: Option<String>) -> Self {
        Self {
            what,
            how,
            kept: false,
        }
    }
}

impl Tip {
    /// For one item. Reads the disk (which folders exist), so build it only
    /// when the tooltip opens.
    pub(super) fn item(item: &Item, size: f64) -> Self {
        let targets = item.targets();
        let more = targets.len().saturating_sub(ROWS);
        let same = targets.windows(2).all(|w| w[0].how == w[1].how);
        let note = same
            .then(|| targets.first().map(|t| t.how.clone()))
            .flatten();
        Self {
            icon: item.icon.clone(),
            title: item.name.clone(),
            size: gb(size),
            rows: targets
                .into_iter()
                .take(ROWS)
                .map(|t| Row::new(tilde(&t.what), (!same).then_some(t.how)))
                .collect(),
            note,
            more,
        }
    }

    /// For the "N more" tile: its items, largest first.
    pub(super) fn group(items: &[Item], mut members: Vec<(usize, f64)>) -> Self {
        members.sort_by(|a, b| b.1.total_cmp(&a.1));
        Self {
            icon: None,
            title: format!("{} more", members.len()),
            size: gb(members.iter().map(|m| m.1).sum()),
            note: None,
            more: members.len().saturating_sub(ROWS),
            rows: members
                .into_iter()
                .take(ROWS)
                .map(|(i, size)| Row::new(items[i].name.clone(), Some(gb(size))))
                .collect(),
        }
    }

    /// For one image name's builds: which stay, and what goes.
    pub(super) fn builds(builds: &Builds) -> Self {
        let kept = |what: String| Row {
            what,
            how: Some("Kept".into()),
            kept: true,
        };
        let shown = ROWS - 3;
        let mut rows: Vec<Row> = builds
            .kept
            .iter()
            .take(shown)
            .map(|(label, why)| kept(format!("{label}, {why}")))
            .collect();
        if builds.kept.len() > shown {
            rows.push(kept(format!(
                "{} more",
                count(builds.kept.len() - shown, "build")
            )));
        }
        rows.push(Row::new(
            count(builds.older, "older build"),
            Some("Removed".into()),
        ));
        if !builds.old.containers.is_empty() {
            rows.push(Row::new(
                format!(
                    "{} on them",
                    count(builds.old.containers.len(), "stopped container")
                ),
                Some("Removed with their logs".into()),
            ));
        }
        Self {
            icon: None,
            title: builds.name.clone(),
            size: size(builds.old.bytes),
            note: None,
            rows,
            more: 0,
        }
    }

    /// Plain rows: what, what happens to it, and whether it stays.
    pub(super) fn rows(
        title: String,
        size: String,
        rows: Vec<(String, Option<String>, bool)>,
    ) -> Self {
        let more = rows.len().saturating_sub(ROWS);
        Self {
            icon: None,
            title,
            size,
            note: None,
            rows: rows
                .into_iter()
                .take(ROWS)
                .map(|(what, how, kept)| Row { what, how, kept })
                .collect(),
            more,
        }
    }

    pub(super) fn view(self, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let muted = rgb(MUTED);
        // Paths keep their end, where they differ.
        let row = |row: &Row| {
            h_flex()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_color(rgb(FG))
                        .child(row.what.clone()),
                )
                .children(row.how.clone().map(|how| {
                    div()
                        .flex_none()
                        .text_color(if row.kept { rgb(OK) } else { muted })
                        .child(how)
                }))
        };
        // Tooltips sit in their own layer, so a margin keeps the shadow and
        // the gap to the cursor inside it.
        div().p_2().font_family(crate::FONT).child(
            v_flex()
                .w(z(380.0))
                .p_3()
                .gap_2()
                .rounded(z(12.0))
                .border_1()
                .border_color(rgb(LINE))
                .bg(rgb(0x1c1c1f))
                .shadow_lg()
                .text_size(z(12.0))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .children(
                            self.icon
                                .clone()
                                .map(|icon| img(icon).size(z(16.0)).flex_none()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(z(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(FG))
                                .child(self.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family("Menlo")
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(FG))
                                .child(self.size.clone()),
                        ),
                )
                .children(
                    self.note
                        .clone()
                        .map(|note| div().text_color(muted).child(note)),
                )
                .child(
                    v_flex()
                        .gap_1p5()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(LINE))
                        .children(self.rows.iter().map(row))
                        .when(self.more > 0, |list| {
                            list.child(
                                div()
                                    .text_color(muted)
                                    .child(format!("and {} more", self.more)),
                            )
                        }),
                ),
        )
    }
}
