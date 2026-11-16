//! Removing apps with their files, from a panel over the card.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{BG, Card, FG, LINE, MUTED, Phase, duo, gb, icon, tilde, tone};
use crate::disk::{self, Category};
use crate::uninstall::{self, App};

#[derive(Default)]
pub(super) struct Apps {
    /// None while the apps are being read.
    list: Option<Vec<App>>,
    /// The app showing its files.
    open: Option<usize>,
    removing: bool,
    /// Why the open app wasn't removed.
    note: Option<String>,
    /// Whether anything was removed, so closing rescans.
    changed: bool,
}

const REMOVE: &str = if cfg!(target_os = "macos") {
    "Move to Trash"
} else {
    "Uninstall"
};

impl Card {
    pub(super) fn show_apps(&mut self, cx: &mut Context<Self>) {
        if self.apps.is_some() || self.phase != Phase::Idle {
            return;
        }
        self.apps = Some(Apps::default());
        let demo = self.demo;
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { uninstall::list(&disk::home(), demo) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(apps) = &mut this.apps {
                    apps.list = Some(list);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close_apps(&mut self) {
        if let Some(apps) = self.apps.take()
            && apps.changed
        {
            self.scan();
        }
    }

    fn remove_app(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(apps) = &mut self.apps else {
            return;
        };
        let Some(app) = apps.list.as_mut().and_then(|list| list.get(i)) else {
            return;
        };
        if app.is_open() {
            apps.note = Some(format!("Quit {} first.", app.name));
            return;
        }
        apps.removing = true;
        apps.note = None;
        let app = app.clone();
        cx.spawn(async move |this, cx| {
            let name = app.name.clone();
            let ok = cx
                .background_executor()
                .spawn(async move { app.remove() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(apps) = &mut this.apps {
                    apps.removing = false;
                    apps.changed = true;
                    if ok {
                        apps.open = None;
                        if let Some(list) = &mut apps.list {
                            list.remove(i);
                        }
                    } else {
                        apps.note = Some(format!("Couldn't remove all of {name}."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_apps(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let apps = self.apps.as_ref()?;
        let muted = rgb(MUTED);

        let body = match &apps.list {
            None => h_flex()
                .py_4()
                .gap_2()
                .child(Spinner::new().small().color(muted.into()))
                .child(div().text_color(muted).child("Reading your apps"))
                .into_any_element(),
            Some(list) if list.is_empty() => div()
                .py_4()
                .text_color(muted)
                .child("No apps you can remove.")
                .into_any_element(),
            Some(list) => div()
                .id("apps-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .border_t_1()
                .border_color(rgb(LINE))
                .children(
                    list.iter()
                        .enumerate()
                        .map(|(i, app)| self.app_row(i, app, apps, cx)),
                )
                .into_any_element(),
        };

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
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Apps"),
                    )
                    .child(
                        Button::new("apps-close")
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .disabled(apps.removing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_apps();
                                cx.notify();
                            })),
                    ),
            )
            .child(body);

        Some(
            div()
                .id("apps")
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

    fn app_row(&self, i: usize, app: &App, apps: &Apps, cx: &mut Context<Self>) -> AnyElement {
        let muted = rgb(MUTED);
        let open = apps.open == Some(i);
        let icon = match &app.icon {
            Some(image) => img(image.clone()).size(px(20.0)).into_any_element(),
            None => duo(icon(Category::Apps), tone(Category::Apps), 16.0).into_any_element(),
        };
        let head = h_flex()
            .id(("app", i))
            .gap_2()
            .py_2()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(apps) = &mut this.apps
                    && !apps.removing
                {
                    apps.open = if apps.open == Some(i) { None } else { Some(i) };
                    apps.note = None;
                }
                cx.notify();
            }))
            .child(
                div()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(app.name.clone()),
            )
            .child(div().text_color(muted).child(gb(app.bytes() as f64)))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size_3p5()
                .text_color(muted),
            );

        let mut row = v_flex().border_b_1().border_color(rgb(LINE)).child(head);
        if open {
            let files = app.files.iter().map(|(path, bytes)| {
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family("Menlo")
                            .text_size(px(11.5))
                            .text_color(rgb(FG))
                            .child(tilde(&path.to_string_lossy())),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(muted)
                            .child(gb(*bytes as f64)),
                    )
            });
            let removing = apps.removing;
            row = row.child(
                v_flex()
                    .gap_2()
                    .pl(px(28.0))
                    .pb_3()
                    .children(files)
                    .children(cfg!(not(target_os = "macos")).then(|| {
                        div()
                            .text_color(muted)
                            .child("Deleted for good, not moved to the Trash.")
                    }))
                    .child(
                        h_flex()
                            .pt_1()
                            .gap_3()
                            .justify_end()
                            .children(
                                apps.note
                                    .clone()
                                    .map(|note| div().flex_1().text_color(muted).child(note)),
                            )
                            .children(removing.then(|| Spinner::new().small().color(muted.into())))
                            .child(
                                Button::new("app-remove")
                                    .primary()
                                    .small()
                                    .label(REMOVE)
                                    .disabled(removing)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_app(i, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        row.into_any_element()
    }
}
