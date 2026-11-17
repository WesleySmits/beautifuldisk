//! Scrolling that glides: wheel steps ease to where they land instead of
//! jumping. Trackpads already scroll smoothly, so they pass straight through.

use std::cell::Cell;
use std::rc::Rc;

use dither_gpui::dither_core::ease::approach;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Card;

/// Wheel notches move this far.
const NOTCH: f32 = 48.0;

#[derive(Default)]
pub(super) struct Scroll {
    at: f32,
    to: f32,
    /// Heights from the last layout, which bound the scroll.
    viewport: Rc<Cell<f32>>,
    content: Rc<Cell<f32>>,
}

impl Scroll {
    fn max(&self) -> f32 {
        (self.content.get() - self.viewport.get()).max(0.0)
    }

    /// Eases toward where the wheel sent it. True while still moving.
    pub(super) fn tick(&mut self, dt: f32) -> bool {
        // Content that shrank can leave it scrolled past the end.
        self.to = self.to.clamp(0.0, self.max());
        self.at = approach(self.at, self.to, dt, 0.04);
        if (self.at - self.to).abs() < 0.5 {
            self.at = self.to;
            false
        } else {
            true
        }
    }

    fn wheel(&mut self, delta: ScrollDelta) {
        match delta {
            ScrollDelta::Pixels(p) => {
                self.to -= f32::from(p.y);
                self.at = self.to;
            }
            ScrollDelta::Lines(l) => self.to -= l.y * NOTCH,
        }
        let max = self.max();
        self.to = self.to.clamp(0.0, max);
        self.at = self.at.clamp(0.0, max);
    }

    /// A clipped area that scrolls `children`. `get` finds this scroll on
    /// the card when the wheel turns.
    pub(super) fn area(
        &self,
        id: &'static str,
        cx: &mut Context<Card>,
        get: fn(&mut Card) -> Option<&mut Scroll>,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let (viewport, content) = (self.viewport.clone(), self.content.clone());
        v_flex()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                if let Some(scroll) = get(this) {
                    scroll.wheel(e.delta);
                }
                cx.notify();
            }))
            .child(measure(move |h| viewport.set(h)))
            .child(
                v_flex()
                    .flex_none()
                    .relative()
                    .top(px(-self.at))
                    .child(measure(move |h| content.set(h)))
                    .children(children),
            )
    }
}

/// An invisible layer that reports its parent's height after layout.
pub(super) fn measure(report: impl Fn(f32) + 'static) -> impl IntoElement {
    canvas(
        move |bounds, _, _| report(f32::from(bounds.size.height)),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}
