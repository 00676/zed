use std::marker::PhantomData;

use gpui2::{px, rgb, Div, Hsla};
use ui::prelude::*;

use crate::story::Story;

/// A reimplementation of the MDN `z-index` example, found here:
/// [https://developer.mozilla.org/en-US/docs/Web/CSS/z-index](https://developer.mozilla.org/en-US/docs/Web/CSS/z-index).
#[derive(IntoAnyElement)]
pub struct ZIndexStory<S: 'static + Send + Sync> {
    state_type: PhantomData<S>,
}

impl<S: 'static + Send + Sync> ZIndexStory<S> {
    pub fn new() -> Self {
        Self {
            state_type: PhantomData,
        }
    }

    fn render(self, _view: &mut S, cx: &mut ViewContext<S>) -> impl IntoAnyElement<S> {
        Story::container(cx)
            .child(Story::title(cx, "z-index"))
            .child(
                div()
                    .flex()
                    .child(
                        div()
                            .w(px(250.))
                            .child(Story::label(cx, "z-index: auto"))
                            .child(ZIndexExample::new(0)),
                    )
                    .child(
                        div()
                            .w(px(250.))
                            .child(Story::label(cx, "z-index: 1"))
                            .child(ZIndexExample::new(1)),
                    )
                    .child(
                        div()
                            .w(px(250.))
                            .child(Story::label(cx, "z-index: 3"))
                            .child(ZIndexExample::new(3)),
                    )
                    .child(
                        div()
                            .w(px(250.))
                            .child(Story::label(cx, "z-index: 5"))
                            .child(ZIndexExample::new(5)),
                    )
                    .child(
                        div()
                            .w(px(250.))
                            .child(Story::label(cx, "z-index: 7"))
                            .child(ZIndexExample::new(7)),
                    ),
            )
    }
}

trait Styles: Styled + Sized {
    // Trailing `_` is so we don't collide with `block` style `StyleHelpers`.
    fn block_(self) -> Self {
        self.absolute()
            .w(px(150.))
            .h(px(50.))
            .text_color(rgb::<Hsla>(0x000000))
    }

    fn blue(self) -> Self {
        self.bg(rgb::<Hsla>(0xe5e8fc))
            .border_5()
            .border_color(rgb::<Hsla>(0x112382))
            .line_height(px(55.))
            // HACK: Simulate `text-align: center`.
            .pl(px(24.))
    }

    fn red(self) -> Self {
        self.bg(rgb::<Hsla>(0xfce5e7))
            .border_5()
            .border_color(rgb::<Hsla>(0xe3a1a7))
            // HACK: Simulate `text-align: center`.
            .pl(px(8.))
    }
}

impl<V: 'static + Send + Sync> Styles for Div<V> {}

#[derive(IntoAnyElement)]
struct ZIndexExample<V: 'static + Send + Sync> {
    view_type: PhantomData<V>,
    z_index: u32,
}

impl<V: 'static + Send + Sync> ZIndexExample<V> {
    pub fn new(z_index: u32) -> Self {
        Self {
            view_type: PhantomData,
            z_index,
        }
    }

    fn render(self, _view: &mut V, cx: &mut ViewContext<V>) -> impl IntoAnyElement<V> {
        div()
            .relative()
            .size_full()
            // Example element.
            .child(
                div()
                    .absolute()
                    .top(px(15.))
                    .left(px(15.))
                    .w(px(180.))
                    .h(px(230.))
                    .bg(rgb::<Hsla>(0xfcfbe5))
                    .text_color(rgb::<Hsla>(0x000000))
                    .border_5()
                    .border_color(rgb::<Hsla>(0xe3e0a1))
                    .line_height(px(215.))
                    // HACK: Simulate `text-align: center`.
                    .pl(px(24.))
                    .z_index(self.z_index)
                    .child(format!(
                        "z-index: {}",
                        if self.z_index == 0 {
                            "auto".to_string()
                        } else {
                            self.z_index.to_string()
                        }
                    )),
            )
            // Blue blocks.
            .child(
                div()
                    .blue()
                    .block_()
                    .top(px(0.))
                    .left(px(0.))
                    .z_index(6)
                    .child("z-index: 6"),
            )
            .child(
                div()
                    .blue()
                    .block_()
                    .top(px(30.))
                    .left(px(30.))
                    .z_index(4)
                    .child("z-index: 4"),
            )
            .child(
                div()
                    .blue()
                    .block_()
                    .top(px(60.))
                    .left(px(60.))
                    .z_index(2)
                    .child("z-index: 2"),
            )
            // Red blocks.
            .child(
                div()
                    .red()
                    .block_()
                    .top(px(150.))
                    .left(px(0.))
                    .child("z-index: auto"),
            )
            .child(
                div()
                    .red()
                    .block_()
                    .top(px(180.))
                    .left(px(30.))
                    .child("z-index: auto"),
            )
            .child(
                div()
                    .red()
                    .block_()
                    .top(px(210.))
                    .left(px(60.))
                    .child("z-index: auto"),
            )
    }
}
