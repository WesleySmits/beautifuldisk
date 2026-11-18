//! The last step before clearing: every path and command, listed, each one
//! ticked so any can be left out.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::docker_panel::size;
use super::scroll::Scroll;
use super::{BG, Card, FG, LINE, MUTED, Phase, gb, tilde};
use crate::clutter::{Chosen, Target};
use crate::ui::z;

/// What the confirmation lists, and which lines are ticked.
pub(super) struct Confirm {
    groups: Vec<Group>,
}

struct Group {
    item: usize,
    targets: Vec<Target>,
    on: Vec<bool>,
}

impl Group {
    fn all(&self) -> bool {
        self.on.iter().all(|&on| on)
    }

    fn any(&self) -> bool {
        self.on.iter().any(|&on| on)
    }
}

impl Confirm {
    /// The items to clear, each with what was unticked.
    fn chosen(&self) -> Vec<Chosen> {
        self.groups
            .iter()
            .filter(|g| g.any())
            .map(|g| {
                let off = || g.targets.iter().zip(&g.on).filter(|(_, on)| !**on);
                Chosen {
                    item: g.item,
                    left_out: off().flat_map(|(t, _)| t.paths.iter().cloned()).collect(),
                    left_out_bytes: off().map(|(t, _)| t.bytes).sum(),
                }
            })
            .collect()
    }
}

impl Card {
    /// Asks before clearing the items `pick` says, from the treemap or the
    /// developer row. Autoplay clears right away.
    pub(super) fn ask_clear(
        &mut self,
        pick: impl Fn(&Self, usize) -> bool,
        cx: &mut Context<Self>,
    ) {
        let Some(items) = &self.items else {
            return;
        };
        let chosen: Vec<usize> = (0..items.len())
            .filter(|&i| pick(self, i) && items[i].left() > 0)
            .collect();
        if chosen.is_empty() || self.phase != Phase::Idle {
            return;
        }
        if self.autoplay {
            self.clear(chosen.into_iter().map(Chosen::whole).collect(), cx);
        } else {
            let groups = chosen
                .into_iter()
                .map(|item| {
                    let targets = items[item].targets();
                    Group {
                        item,
                        on: vec![true; targets.len()],
                        targets,
                    }
                })
                .collect();
            self.hover = None;
            self.hover_item = None;
            self.dev_hover = false;
            self.confirm = Some(Confirm { groups });
            self.confirm_scroll = Scroll::default();
        }
    }

    pub(super) fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm.as_ref()?;
        let items = self.items.as_ref()?;
        let muted = rgb(MUTED);
        // Whole items count what is left of them; partly ticked ones what
        // their ticked lines came to at the scan.
        let total: f64 = confirm
            .groups
            .iter()
            .map(|g| {
                let shown = self.item_shown[g.item];
                if g.all() {
                    shown
                } else {
                    let on: u64 = g
                        .targets
                        .iter()
                        .zip(&g.on)
                        .filter(|(_, on)| **on)
                        .map(|(t, _)| t.bytes)
                        .sum();
                    (on as f64).min(shown)
                }
            })
            .sum();
        let anything = confirm.groups.iter().any(Group::any);

        let groups: Vec<AnyElement> = confirm
            .groups
            .iter()
            .enumerate()
            .map(|(g, group)| {
                let item = &items[group.item];
                // One line is ticked with its item.
                let several = group.targets.len() > 1;
                let targets = group.targets.iter().enumerate().map(|(t, target)| {
                    let on = group.on[t];
                    h_flex()
                        .id(("confirm-target", g * 10_000 + t))
                        .items_start()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(group) =
                                this.confirm.as_mut().and_then(|c| c.groups.get_mut(g))
                            {
                                group.on[t] = !group.on[t];
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .pt(z(1.0))
                                .when(several, |d| d.child(check(on, false)))
                                .when(!several, |d| d.w(z(15.0))),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .when(!on, |row| row.opacity(0.45))
                                .child(
                                    div()
                                        .font_family("Menlo")
                                        .text_size(z(12.0))
                                        .text_color(rgb(FG))
                                        .child(tilde(&target.what)),
                                )
                                .child(
                                    div()
                                        .text_size(z(12.0))
                                        .text_color(muted)
                                        .child(target.how.clone()),
                                ),
                        )
                        .when(several, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(z(12.0))
                                    .text_color(muted)
                                    .child(size(target.bytes)),
                            )
                        })
                });
                v_flex()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        h_flex()
                            .id(("confirm-item", g))
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(group) =
                                    this.confirm.as_mut().and_then(|c| c.groups.get_mut(g))
                                {
                                    let on = !group.all();
                                    group.on.fill(on);
                                    cx.notify();
                                }
                            }))
                            .child(check(group.any(), group.any() && !group.all()))
                            .children(item.icon.clone().map(|icon| img(icon).size(z(16.0))))
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(item.name.clone()),
                            )
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(size(self.item_shown[group.item] as u64)),
                            ),
                    )
                    .child(v_flex().gap_2().pl(z(24.0)).children(targets))
                    .into_any_element()
            })
            .collect();

        let panel = v_flex()
            .w_full()
            .max_w(z(640.0))
            .max_h_full()
            .p_5()
            .gap_3()
            .rounded(z(12.0))
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(0x1c1c1f))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(z(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Clear {}?", gb(total))),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child("These are deleted for good, not moved to the Trash."),
                    ),
            )
            .child(
                self.confirm_scroll
                    .area(
                        "confirm-list",
                        cx,
                        |this| Some(&mut this.confirm_scroll),
                        groups,
                    )
                    .border_t_1()
                    .border_color(rgb(LINE)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("confirm-clear")
                            .primary()
                            .small()
                            .label("Clear")
                            .disabled(!anything)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(confirm) = this.confirm.take() {
                                    this.clear(confirm.chosen(), cx);
                                }
                                cx.notify();
                            })),
                    ),
            );

        Some(
            div()
                .id("confirm")
                .absolute()
                .inset_0()
                .p_6()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba((BG << 8) | 0xd8))
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }
}

/// A checkbox: filled with a tick when on, with a dash when only partly on.
pub(super) fn check(on: bool, partly: bool) -> Div {
    let mark = if partly {
        div().w(z(8.0)).h(z(2.0)).rounded_full().bg(rgb(BG))
    } else {
        div().child(
            svg()
                .path(IconName::Check.path())
                .size(z(11.0))
                .text_color(rgb(BG)),
        )
    };
    div()
        .size(z(15.0))
        .flex_none()
        .rounded(z(4.0))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .when(on, |c| c.bg(rgb(FG)).border_color(rgb(FG)).child(mark))
        .when(!on, |c| c.border_color(rgb(0x44444a)))
}
