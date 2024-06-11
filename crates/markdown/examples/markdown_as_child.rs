use assets::Assets;
use gpui::*;
use language::{language_settings::AllLanguageSettings, LanguageRegistry};
use markdown::{Markdown, MarkdownStyle};
use node_runtime::FakeNodeRuntime;
use settings::SettingsStore;
use std::sync::Arc;
use theme::LoadThemes;
use ui::div;
use ui::prelude::*;

const MARKDOWN_EXAMPLE: &'static str = r#"
this text should be selectable

wow so cool

## Heading 2
"#;
pub fn main() {
    env_logger::init();

    App::new().with_assets(Assets).run(|cx| {
        let store = SettingsStore::test(cx);
        cx.set_global(store);
        language::init(cx);
        SettingsStore::update(cx, |store, cx| {
            store.update_user_settings::<AllLanguageSettings>(cx, |_| {});
        });
        cx.bind_keys([KeyBinding::new("cmd-c", markdown::Copy, None)]);

        let node_runtime = FakeNodeRuntime::new();
        let language_registry = Arc::new(LanguageRegistry::new(
            Task::ready(()),
            cx.background_executor().clone(),
        ));
        languages::init(language_registry.clone(), node_runtime, cx);
        theme::init(LoadThemes::JustBase, cx);
        Assets.load_fonts(cx).unwrap();

        cx.activate(true);
        cx.open_window(WindowOptions::default(), |cx| {
            cx.new_view(|cx| {
                let markdown_style = MarkdownStyle {
                    code_block: gpui::TextStyleRefinement {
                        font_family: Some("Zed Mono".into()),
                        color: Some(cx.theme().colors().editor_foreground),
                        background_color: Some(cx.theme().colors().editor_background),
                        ..Default::default()
                    },
                    inline_code: gpui::TextStyleRefinement {
                        font_family: Some("Zed Mono".into()),
                        // @nate: Could we add inline-code specific styles to the theme?
                        color: Some(cx.theme().colors().editor_foreground),
                        background_color: Some(cx.theme().colors().editor_background),
                        ..Default::default()
                    },
                    rule_color: Color::Muted.color(cx),
                    block_quote_border_color: Color::Muted.color(cx),
                    block_quote: gpui::TextStyleRefinement {
                        color: Some(Color::Muted.color(cx)),
                        ..Default::default()
                    },
                    link: gpui::TextStyleRefinement {
                        color: Some(Color::Accent.color(cx)),
                        underline: Some(gpui::UnderlineStyle {
                            thickness: px(1.),
                            color: Some(Color::Accent.color(cx)),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                    syntax: cx.theme().syntax().clone(),
                    selection_background_color: {
                        let mut selection = cx.theme().players().local().selection;
                        selection.fade_out(0.7);
                        selection
                    },
                    pad_blocks: true,
                };
                let markdown = cx.new_view(|cx| {
                    Markdown::new(MARKDOWN_EXAMPLE.into(), markdown_style, None, cx)
                });

                HelloWorld { markdown }
            })
        });
    });
}
struct HelloWorld {
    markdown: View<Markdown>,
}

impl Render for HelloWorld {
    fn render(&mut self, _cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .flex()
            .bg(rgb(0x2e7d32))
            .size(Length::Definite(Pixels(300.0).into()))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border_1()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(self.markdown.clone())
    }
}