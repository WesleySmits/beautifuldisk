//! The last step before clearing: every path and command, listed.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::scroll::Scroll;
use super::{BG, Card, FG, LINE, MUTED, Phase, gb, tilde};

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
            self.clear(chosen, cx);
        } else {
            self.hover = None;
            self.hover_item = None;
            self.dev_hover = false;
            self.confirm = Some(chosen);
            self.confirm_scroll = Scroll::default();
        }
    }

    pub(super) fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let chosen = self.confirm.clone()?;
        let items = self.items.as_ref()?;
        let muted = rgb(MUTED);
        let total: f64 = chosen.iter().map(|&i| self.item_shown[i]).sum();

        let groups: Vec<AnyElement> = chosen
            .iter()
            .map(|&i| {
                let item = &items[i];
                let targets = item.targets().into_iter().map(|t| {
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .font_family("Menlo")
                                .text_size(px(11.5))
                                .text_color(rgb(FG))
                                .child(tilde(&t.what)),
                        )
                        .child(div().text_size(px(11.5)).text_color(muted).child(t.how))
                });
                v_flex()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        h_flex()
                            .gap_2()
                            .children(item.icon.clone().map(|icon| img(icon).size(px(16.0))))
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(item.name.clone()),
                            )
                            .child(div().text_color(muted).child(gb(self.item_shown[i]))),
                    )
                    .child(v_flex().gap_2().pl(px(24.0)).children(targets))
                    .into_any_element()
            })
            .collect();

        let panel = v_flex()
            .w_full()
            .max_w(px(640.0))
            .max_h_full()
            .p_5()
            .gap_3()
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(0x1c1c1f))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(20.0))
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(chosen) = this.confirm.take() {
                                    this.clear(chosen, cx);
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
