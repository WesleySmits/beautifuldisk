//! Linux: a header button for a newer release. macOS updates through
//! Sparkle and its menu item instead, so there this is empty.

#[cfg(not(target_os = "macos"))]
pub(super) use linux::Update;

/// Never made on macOS.
#[cfg(target_os = "macos")]
pub(super) enum Update {}

#[cfg(target_os = "macos")]
impl super::Card {
    pub(super) fn check_update(&mut self, _cx: &mut gpui_kit::Context<Self>) {}

    pub(super) fn update_button(
        &self,
        _cx: &mut gpui_kit::Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
mod linux {
    use std::path::PathBuf;

    use gpui_kit::assets::IconName;
    use gpui_kit::component::button::{Button, ButtonVariants};
    use gpui_kit::component::{Disableable, Sizable};
    use gpui_kit::prelude::*;
    use gpui_kit::*;

    use super::super::Card;

    pub(in crate::card) enum Update {
        Available(String),
        Installing,
        /// Installed; starting this path runs the new version.
        Ready(PathBuf),
        Failed,
    }

    impl Card {
        pub(in crate::card) fn check_update(&mut self, cx: &mut Context<Self>) {
            cx.spawn(async move |this, cx| {
                let newer = cx
                    .background_executor()
                    .spawn(async { crate::update::newer() })
                    .await;
                if let Some(version) = newer {
                    let _ = this.update(cx, |this, cx| {
                        this.update = Some(Update::Available(version));
                        cx.notify();
                    });
                }
            })
            .detach();
        }

        fn install_update(&mut self, cx: &mut Context<Self>) {
            self.update = Some(Update::Installing);
            cx.spawn(async move |this, cx| {
                let installed = cx
                    .background_executor()
                    .spawn(async { crate::update::install() })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.update = Some(installed.map_or(Update::Failed, Update::Ready));
                    cx.notify();
                });
            })
            .detach();
        }

        pub(in crate::card) fn update_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
            let update = self.update.as_ref()?;
            let label = match update {
                Update::Available(version) => format!("Update to {version}"),
                Update::Installing => "Updating".into(),
                Update::Ready(_) => "Restart to update".into(),
                Update::Failed => "Couldn't update".into(),
            };
            let busy = matches!(update, Update::Installing | Update::Failed);
            Some(
                Button::new("update")
                    .primary()
                    .small()
                    .icon(IconName::ArrowDown)
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| match &this.update {
                        Some(Update::Available(_)) => this.install_update(cx),
                        Some(Update::Ready(path)) => {
                            if std::process::Command::new(path).spawn().is_ok() {
                                cx.quit();
                            }
                        }
                        _ => {}
                    }))
                    .into_any_element(),
            )
        }
    }
}
