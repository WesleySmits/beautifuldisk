//! The treemap's hover card: what clearing a tile removes.

use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{FG, LINE, MUTED, gb, tilde};
use crate::clutter::Item;

/// Rows shown before the rest are counted.
const ROWS: usize = 6;

pub(super) struct Tip {
    icon: Option<Arc<Image>>,
    title: String,
    size: f64,
    /// What happens to every row, when it is the same for all.
    note: Option<String>,
    /// Path or name, and what happens to it or its size.
    rows: Vec<(String, Option<String>)>,
    more: usize,
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
            size,
            rows: targets
                .into_iter()
                .take(ROWS)
                .map(|t| (tilde(&t.what), (!same).then_some(t.how)))
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
            size: members.iter().map(|m| m.1).sum(),
            note: None,
            more: members.len().saturating_sub(ROWS),
            rows: members
                .into_iter()
                .take(ROWS)
                .map(|(i, size)| (items[i].name.clone(), Some(gb(size))))
                .collect(),
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
        let row = |(what, how): &(String, Option<String>)| {
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
                        .child(what.clone()),
                )
                .children(
                    how.clone()
                        .map(|how| div().flex_none().text_color(muted).child(how)),
                )
        };
        // Tooltips sit in their own layer, so a margin keeps the shadow and
        // the gap to the cursor inside it.
        div().p_2().child(
            v_flex()
                .w(px(380.0))
                .p_3()
                .gap_2()
                .rounded(px(12.0))
                .border_1()
                .border_color(rgb(LINE))
                .bg(rgb(0x1c1c1f))
                .shadow_lg()
                .text_size(px(12.0))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .children(
                            self.icon
                                .clone()
                                .map(|icon| img(icon).size(px(16.0)).flex_none()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.0))
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
                                .child(gb(self.size)),
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
