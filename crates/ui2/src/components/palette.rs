use std::marker::PhantomData;

use crate::prelude::*;
use crate::theme::theme;
use crate::{h_stack, v_stack, Keybinding, Label, LabelColor};

#[derive(Element)]
pub struct Palette<S: 'static + Send + Sync + Clone> {
    state_type: PhantomData<S>,
    scroll_state: ScrollState,
    input_placeholder: SharedString,
    empty_string: SharedString,
    items: Vec<PaletteItem<S>>,
    default_order: OrderMethod,
}

impl<S: 'static + Send + Sync + Clone> Palette<S> {
    pub fn new(scroll_state: ScrollState) -> Self {
        Self {
            state_type: PhantomData,
            scroll_state,
            input_placeholder: "Find something...".into(),
            empty_string: "No items found.".into(),
            items: vec![],
            default_order: OrderMethod::default(),
        }
    }

    pub fn items(mut self, items: Vec<PaletteItem<S>>) -> Self {
        self.items = items;
        self
    }

    pub fn placeholder(mut self, input_placeholder: impl Into<SharedString>) -> Self {
        self.input_placeholder = input_placeholder.into();
        self
    }

    pub fn empty_string(mut self, empty_string: impl Into<SharedString>) -> Self {
        self.empty_string = empty_string.into();
        self
    }

    // TODO: Hook up sort order
    pub fn default_order(mut self, default_order: OrderMethod) -> Self {
        self.default_order = default_order;
        self
    }

    fn render(&mut self, _view: &mut S, cx: &mut ViewContext<S>) -> impl Element<ViewState = S> {
        let theme = theme(cx);

        v_stack()
            .w_96()
            .rounded_lg()
            .bg(theme.lowest.base.default.background)
            .border()
            .border_color(theme.lowest.base.default.border)
            .child(
                v_stack()
                    .gap_px()
                    .child(v_stack().py_0p5().px_1().child(div().px_2().py_0p5().child(
                        Label::new(self.input_placeholder.clone()).color(LabelColor::Placeholder),
                    )))
                    .child(div().h_px().w_full().bg(theme.lowest.base.default.border))
                    .child(
                        v_stack()
                            .py_0p5()
                            .px_1()
                            .grow()
                            .max_h_96()
                            .overflow_y_scroll(self.scroll_state.clone())
                            .children(
                                vec![if self.items.is_empty() {
                                    Some(
                                        h_stack().justify_between().px_2().py_1().child(
                                            Label::new(self.empty_string.clone())
                                                .color(LabelColor::Muted),
                                        ),
                                    )
                                } else {
                                    None
                                }]
                                .into_iter()
                                .flatten(),
                            )
                            .children(self.items.iter().map(|item| {
                                h_stack()
                                    .justify_between()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_lg()
                                    .hover(|style| style.bg(theme.lowest.base.hovered.background))
                                    // .active()
                                    // .fill(theme.lowest.base.pressed.background)
                                    .child(item.clone())
                            })),
                    ),
            )
    }
}

#[derive(Element, Clone)]
pub struct PaletteItem<S: 'static + Send + Sync + Clone> {
    pub label: SharedString,
    pub sublabel: Option<SharedString>,
    pub keybinding: Option<Keybinding<S>>,
}

impl<S: 'static + Send + Sync + Clone> PaletteItem<S> {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            sublabel: None,
            keybinding: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn sublabel(mut self, sublabel: impl Into<Option<SharedString>>) -> Self {
        self.sublabel = sublabel.into();
        self
    }

    pub fn keybinding<K>(mut self, keybinding: K) -> Self
    where
        K: Into<Option<Keybinding<S>>>,
    {
        self.keybinding = keybinding.into();
        self
    }

    fn render(&mut self, _view: &mut S, cx: &mut ViewContext<S>) -> impl Element<ViewState = S> {
        let theme = theme(cx);

        div()
            .flex()
            .flex_row()
            .grow()
            .justify_between()
            .child(
                v_stack()
                    .child(Label::new(self.label.clone()))
                    .children(self.sublabel.clone().map(|sublabel| Label::new(sublabel))),
            )
            .children(self.keybinding.clone())
    }
}

#[cfg(feature = "stories")]
pub use stories::*;

#[cfg(feature = "stories")]
mod stories {
    use crate::{ModifierKeys, Story};

    use super::*;

    #[derive(Element)]
    pub struct PaletteStory<S: 'static + Send + Sync + Clone> {
        state_type: PhantomData<S>,
    }

    impl<S: 'static + Send + Sync + Clone> PaletteStory<S> {
        pub fn new() -> Self {
            Self {
                state_type: PhantomData,
            }
        }

        fn render(
            &mut self,
            _view: &mut S,
            cx: &mut ViewContext<S>,
        ) -> impl Element<ViewState = S> {
            Story::container(cx)
                .child(Story::title_for::<_, Palette<S>>(cx))
                .child(Story::label(cx, "Default"))
                .child(Palette::new(ScrollState::default()))
                .child(Story::label(cx, "With Items"))
                .child(
                    Palette::new(ScrollState::default())
                        .placeholder("Execute a command...")
                        .items(vec![
                            PaletteItem::new("theme selector: toggle").keybinding(
                                Keybinding::new_chord(
                                    ("k".to_string(), ModifierKeys::new().command(true)),
                                    ("t".to_string(), ModifierKeys::new().command(true)),
                                ),
                            ),
                            PaletteItem::new("assistant: inline assist").keybinding(
                                Keybinding::new(
                                    "enter".to_string(),
                                    ModifierKeys::new().command(true),
                                ),
                            ),
                            PaletteItem::new("assistant: quote selection").keybinding(
                                Keybinding::new(">".to_string(), ModifierKeys::new().command(true)),
                            ),
                            PaletteItem::new("assistant: toggle focus").keybinding(
                                Keybinding::new("?".to_string(), ModifierKeys::new().command(true)),
                            ),
                            PaletteItem::new("auto update: check"),
                            PaletteItem::new("auto update: view release notes"),
                            PaletteItem::new("branches: open recent").keybinding(Keybinding::new(
                                "b".to_string(),
                                ModifierKeys::new().command(true).alt(true),
                            )),
                            PaletteItem::new("chat panel: toggle focus"),
                            PaletteItem::new("cli: install"),
                            PaletteItem::new("client: sign in"),
                            PaletteItem::new("client: sign out"),
                            PaletteItem::new("editor: cancel").keybinding(Keybinding::new(
                                "escape".to_string(),
                                ModifierKeys::new(),
                            )),
                        ]),
                )
        }
    }
}
