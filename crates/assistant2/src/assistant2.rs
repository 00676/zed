mod completion_provider;

use anyhow::Result;
use client::Client;
use completion_provider::*;
use editor::{Editor, EditorElement, EditorEvent, EditorStyle};
use futures::{channel::oneshot, Future, FutureExt as _, StreamExt};
use gpui::{
    hsla, list, prelude::*, AnyElement, AppContext, FocusHandle, Global, ImageSource,
    ListAlignment, ListState, Model, Render, SharedUri, Task, TextStyle, View,
};
use language::{language_settings::SoftWrap, LanguageRegistry};
use project::Fs;
use rich_text::RichText;
use semantic_index::ProjectIndex;
use serde::Deserialize;
use settings::Settings;
use std::{cmp, sync::Arc};
use theme::ThemeSettings;
use ui::{
    popover_menu, prelude::*, Avatar, ButtonLike, CollapsibleContainer, Color, ContextMenu, Tooltip,
};
use util::ResultExt;

// gpui::actions!(assistant, [Submit]);

#[derive(Eq, PartialEq, Copy, Clone, Deserialize)]
pub struct Submit(SubmitMode);

/// There are multiple different ways to submit a model request, represented by this enum.
#[derive(Eq, PartialEq, Copy, Clone, Deserialize)]
pub enum SubmitMode {
    /// Only include the conversation.
    Simple,
    /// Send the current file as context.
    CurrentFile,
    /// Search the codebase and send relevant excerpts.
    Codebase,
}

gpui::impl_actions!(assistant, [Submit]);

pub fn init(client: Arc<Client>, cx: &mut AppContext) {
    cx.set_global(CompletionProvider::new(CloudCompletionProvider::new(
        client,
    )));
}

pub struct AssistantPanel {
    #[allow(dead_code)]
    language_registry: Arc<LanguageRegistry>,
    #[allow(dead_code)]
    project_index: Model<ProjectIndex>,
    #[allow(dead_code)]
    fs: Arc<dyn Fs>,
    chat: View<AssistantChat>,
}

impl AssistantPanel {
    pub fn new(
        language_registry: Arc<LanguageRegistry>,
        project_index: Model<ProjectIndex>,
        fs: Arc<dyn Fs>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let chat = cx.new_view(|cx| {
            AssistantChat::new(
                language_registry.clone(),
                project_index.clone(),
                fs.clone(),
                cx,
            )
        });
        Self {
            language_registry,
            project_index,
            fs,
            chat,
        }
    }
}

impl Render for AssistantPanel {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .p_2()
            .bg(cx.theme().colors().background)
            .child(self.chat.clone())
    }
}

struct AssistantChat {
    model: String,
    messages: Vec<ChatMessage>,
    list_state: ListState,
    language_registry: Arc<LanguageRegistry>,
    project_index: Model<ProjectIndex>,
    fs: Arc<dyn Fs>,
    next_message_id: MessageId,
    pending_completion: Option<Task<()>>,
}

impl AssistantChat {
    fn new(
        language_registry: Arc<LanguageRegistry>,
        project_index: Model<ProjectIndex>,
        fs: Arc<dyn Fs>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let model = CompletionProvider::get(cx).default_model();
        let view = cx.view().downgrade();
        let list_state = ListState::new(
            0,
            ListAlignment::Bottom,
            px(1024.),
            move |ix, cx: &mut WindowContext| {
                view.update(cx, |this, cx| this.render_message(ix, cx))
                    .unwrap()
            },
        );

        let mut this = Self {
            model,
            messages: Vec::new(),
            list_state,
            language_registry,
            project_index,
            fs,
            next_message_id: MessageId(0),
            pending_completion: None,
        };
        this.push_new_user_message(true, cx);
        this
    }

    fn focused_message_id(&self, cx: &WindowContext) -> Option<MessageId> {
        self.messages.iter().find_map(|message| match message {
            ChatMessage::User(message) => message
                .body
                .focus_handle(cx)
                .contains_focused(cx)
                .then_some(message.id),
            ChatMessage::Assistant(_) => None,
        })
    }

    fn submit(&mut self, Submit(mode): &Submit, cx: &mut ViewContext<Self>) {
        let Some(focused_message_id) = self.focused_message_id(cx) else {
            log::error!("unexpected state: no user message editor is focused.");
            return;
        };

        self.truncate_messages(focused_message_id, cx);
        self.push_new_assistant_message(cx);

        let populate = self.populate_context_on_submit(focused_message_id, mode, cx);

        self.pending_completion = Some(cx.spawn(|this, mut cx| async move {
            let complete = async {
                populate.await?;

                let completion = this.update(&mut cx, |this, cx| {
                    CompletionProvider::get(cx).complete(
                        this.model.clone(),
                        this.completion_messages(cx),
                        Vec::new(),
                        1.0,
                    )
                });

                let mut stream = completion?.await?;

                let mut body = String::new();

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    this.update(&mut cx, |this, cx| {
                        if let Some(ChatMessage::Assistant(AssistantMessage {
                            body: message_body,
                            ..
                        })) = this.messages.last_mut()
                        {
                            body.push_str(&chunk);
                            *message_body =
                                RichText::new(body.clone(), &[], &this.language_registry);
                            cx.notify();
                        } else {
                            unreachable!()
                        }
                    })?;
                }

                anyhow::Ok(())
            }
            .await;

            this.update(&mut cx, |this, cx| {
                if let Err(error) = complete {
                    if let Some(ChatMessage::Assistant(AssistantMessage {
                        error: message_error,
                        ..
                    })) = this.messages.last_mut()
                    {
                        message_error.replace(SharedString::from(error.to_string()));
                        cx.notify();
                    } else {
                        unreachable!()
                    }
                }

                let focus = this
                    .user_message(focused_message_id)
                    .body
                    .focus_handle(cx)
                    .contains_focused(cx);
                this.push_new_user_message(focus, cx);
            })
            .log_err();
        }));
    }

    /// Set up the query designed for the semantic index, based on previous conversation
    fn setup_query(&self, cx: &mut ViewContext<Self>) -> Task<Result<String>> {
        // Let's try another approach where we take the user's previous messages and turn that into a query
        // by calling for a completion.

        // For now, we'll set up a summary request message, where we tell the model we need something simple to summarize

        let mut query_creation_messages = self.completion_messages(cx);

        query_creation_messages.push(CompletionMessage {
                role: CompletionRole::System,
                body: r#"
                    Turn the user's query into a single search string that can be used to search for code base snippets relevant to the user's query. Everything you respond with will be fed directly to a semantic index.

                    ## Example

                    **User**: How can I create a component in GPUI that works like a `<details>` / `<summary>` pair in HTML?

                    GPUI create component like HTML details summary example
                    "#.into(),
            });

        let query = CompletionProvider::get(cx).complete(
            self.model.clone(),
            query_creation_messages,
            Vec::new(),
            1.0,
        );

        cx.spawn(|_, _| async move {
            let mut stream = query.await?;

            // todo!(): Show the query in the UI as part of the context view
            let mut query = String::new();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                query.push_str(&chunk);
            }

            dbg!(&query);

            anyhow::Ok(query)
        })
    }

    // Returns a oneshot channel which resolves to true when the context is successfully populated.
    fn populate_context_on_submit(
        &mut self,
        submitted_id: MessageId,
        mode: &SubmitMode,
        cx: &mut ViewContext<Self>,
    ) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();

        match mode {
            SubmitMode::Simple => {
                tx.send(true).ok();
            }
            SubmitMode::CurrentFile => {
                tx.send(true).ok();
            }
            SubmitMode::Codebase => {
                self.user_message(submitted_id).contexts.clear();

                let query = self.setup_query(cx);

                let project_index = self.project_index.clone();
                let fs = self.fs.clone();

                self.user_message(submitted_id)
                    .contexts
                    .push(AssistantContext::Codebase(cx.new_view(|cx| {
                        CodebaseContext::new(query, tx, project_index, fs, cx)
                    })));
            }
        }

        rx
    }

    fn user_message(&mut self, message_id: MessageId) -> &mut UserMessage {
        self.messages
            .iter_mut()
            .find_map(|message| match message {
                ChatMessage::User(user_message) if user_message.id == message_id => {
                    Some(user_message)
                }
                _ => None,
            })
            .expect("User message not found")
    }

    fn push_new_user_message(&mut self, focus: bool, cx: &mut ViewContext<Self>) {
        let id = self.next_message_id.post_inc();
        let body = cx.new_view(|cx| {
            let mut editor = Editor::auto_height(80, cx);
            editor.set_soft_wrap_mode(SoftWrap::EditorWidth, cx);
            editor.set_placeholder_text("Send a message...", cx);

            if focus {
                cx.focus_self();
            }
            editor
        });
        let _subscription = cx.subscribe(&body, move |this, editor, event, cx| match event {
            EditorEvent::SelectionsChanged { .. } => {
                if editor.read(cx).is_focused(cx) {
                    let (message_ix, message) = this
                        .messages
                        .iter()
                        .enumerate()
                        .find_map(|(ix, message)| match message {
                            ChatMessage::User(user_message) if user_message.id == id => {
                                Some((ix, user_message))
                            }
                            _ => None,
                        })
                        .expect("user message not found");
                    message.body.update(cx, |body, cx| {
                        if let Some(editor_style) = body.style() {
                            let row = body.selections.newest_display(cx).head().row();
                            let line_height =
                                editor_style.text.line_height_in_pixels(cx.rem_size());
                            let row_y = row as f32 * line_height;
                            this.list_state.scroll_to_fit(
                                message_ix,
                                row_y,
                                row_y + 5. * line_height,
                            );
                        }
                    });
                }
            }
            _ => {}
        });
        let message = ChatMessage::User(UserMessage {
            id,
            body,
            contexts: Vec::new(),
            _subscription,
        });
        self.push_message(message, cx);
    }

    fn push_new_assistant_message(&mut self, cx: &mut ViewContext<Self>) {
        let message = ChatMessage::Assistant(AssistantMessage {
            id: self.next_message_id.post_inc(),
            body: RichText::default(),
            error: None,
        });
        self.push_message(message, cx);
    }

    fn push_message(&mut self, message: ChatMessage, cx: &mut ViewContext<Self>) {
        let old_len = self.messages.len();
        let focus_handle = Some(message.focus_handle(cx));
        self.messages.push(message);
        self.list_state
            .splice_focusable(old_len..old_len, focus_handle);
        cx.notify();
    }

    fn truncate_messages(&mut self, last_message_id: MessageId, cx: &mut ViewContext<Self>) {
        if let Some(index) = self.messages.iter().position(|message| match message {
            ChatMessage::User(message) => message.id == last_message_id,
            ChatMessage::Assistant(message) => message.id == last_message_id,
        }) {
            self.list_state.splice(index + 1..self.messages.len(), 0);
            self.messages.truncate(index + 1);
            cx.notify();
        }
    }

    fn render_error(
        &self,
        error: Option<SharedString>,
        _ix: usize,
        cx: &mut ViewContext<Self>,
    ) -> AnyElement {
        let theme = cx.theme();

        if let Some(error) = error {
            div()
                .py_1()
                .px_2()
                .neg_mx_1()
                .rounded_md()
                .border()
                .border_color(theme.status().error_border)
                // .bg(theme.status().error_background)
                .text_color(theme.status().error)
                .child(error.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }

    fn render_message(&self, ix: usize, cx: &mut ViewContext<Self>) -> AnyElement {
        let is_last = ix == self.messages.len() - 1;

        match &self.messages[ix] {
            // ChatMessage::User(UserMessage { body, contexts, .. }) => div()
            //     // .when(!is_last, |element| element.mb_3())
            //     .child(div().h_3().w_full())
            //     .child(
            //         v_flex()
            //             .group("")
            //             .bg(hsla(1.0, 1.0, 1.0, 0.06))
            //             .rounded_md()
            //             .px_2p5()
            //             .py_1p5()
            //             .child(
            //                 h_flex()
            //                     .justify_between()
            //                     .child(
            //                         div()
            //                             .mb_0p5()
            //                             .child(Label::new("You").color(Color::Default)),
            //                     )
            //                     .child(
            //                         h_flex()
            //                             .mr_1()
            //                             .child(
            //                                 h_flex().visible_on_hover("").gap_1().child(
            //                                     IconButton::new("copy_text", IconName::Copy)
            //                                         .icon_size(IconSize::Small)
            //                                         .icon_color(Color::Muted),
            //                                 ),
            //                             )
            //                             .child(
            //                                 IconButton::new("menu", IconName::Ellipsis)
            //                                     .icon_size(IconSize::Small)
            //                                     .icon_color(Color::Muted),
            //                             ),
            //                     ),
            //             )
            //             .child(
            //                 div()
            //                     .on_action(cx.listener(Self::submit))
            //                     .mx_1()
            //                     .mb_1()
            //                     .p_1()
            //                     .rounded_md()
            //                     .text_color(cx.theme().colors().editor_foreground)
            //                     .font(ThemeSettings::get_global(cx).buffer_font.clone())
            //                     .hover(|this| this.bg(cx.theme().colors().editor_background))
            //                     // .child(body.read(cx).set_read_only(true))
            //                     .child(body.clone())
            //                     .children(contexts.iter().map(|context| context.render(cx))),
            //             ),
            //     )
            //     .into_any(),

            // Composer variant
            ChatMessage::User(UserMessage { body, contexts, .. }) => div()
                .child(div().h_3().w_full())
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .justify_start()
                        .child(
                            div()
                                .pt_0p5()
                                .child(
                                Avatar::new(
                                    "https://encrypted-tbn0.gstatic.com/images?q=tbn:ANd9GcRni2qBxMYI5UwXBrhb5Kds8BamU5DSwjI1bHvhr3S_Pq58Lp4M".to_string()
                                ).size(rems(1.25))
                            ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .w_full()
                                .group("")
                                .bg(cx.theme().colors().editor_background)
                                .rounded_md()
                                .p_2()
                                .child(
                                    div()
                                        .on_action(cx.listener(Self::submit))
                                        .text_color(cx.theme().colors().editor_foreground)
                                        .font(ThemeSettings::get_global(cx).ui_font.clone())
                                        .child(body.clone())
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            h_flex().gap_2().child(self.render_model_dropdown(cx))
                                        )
                                        .child(
                                            h_flex()
                                                .mr_1()
                                                .child(
                                                    h_flex().visible_on_hover("").gap_1().child(
                                                        IconButton::new(
                                                            "copy_text",
                                                            IconName::Copy,
                                                        )
                                                        .icon_size(IconSize::Small)
                                                        .icon_color(Color::Muted),
                                                    ),
                                                )
                                                .child(
                                                    Button::new("send_message", "Send")
                                                        .icon(IconName::Return).icon_color(Color::Muted).icon_position(IconPosition::Start).style(ButtonStyle::Filled)
                                                ),
                                        ),
                                )
                        ),
                )
                .into_any(),
            ChatMessage::Assistant(AssistantMessage { id, body, error }) => div()
                // .when(!is_last, |element| element.mb_3())
                .child(div().h_3().w_full())
                .child(
                    v_flex()
                        .bg(hsla(1.0, 1.0, 1.0, 0.06))
                        .child(
                            div()
                                .p_2()
                                .child(Label::new("Assistant").color(Color::Modified)),
                        )
                        .child(div().p_2().child(body.element(ElementId::from(id.0), cx)))
                        .child(self.render_error(error.clone(), ix, cx)),
                )
                .into_any(),
        }
    }

    fn completion_messages(&self, cx: &WindowContext) -> Vec<CompletionMessage> {
        let mut completion_messages = Vec::new();

        for message in &self.messages {
            match message {
                ChatMessage::User(UserMessage { body, contexts, .. }) => {
                    // setup context for model
                    contexts.iter().for_each(|context| {
                        completion_messages.extend(context.completion_messages(cx))
                    });

                    // Show user's message last so that the assistant is grounded in the user's request
                    completion_messages.push(CompletionMessage {
                        role: CompletionRole::User,
                        body: body.read(cx).text(cx),
                    });
                }
                ChatMessage::Assistant(AssistantMessage { body, .. }) => {
                    completion_messages.push(CompletionMessage {
                        role: CompletionRole::Assistant,
                        body: body.text.to_string(),
                    });
                }
            }
        }

        completion_messages
    }

    fn render_model_dropdown(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let this = cx.view().downgrade();
        div().h_flex().justify_end().child(
            div().w_32().child(
                popover_menu("user-menu")
                    .menu(move |cx| {
                        ContextMenu::build(cx, |mut menu, cx| {
                            for model in CompletionProvider::get(cx).available_models() {
                                menu = menu.custom_entry(
                                    {
                                        let model = model.clone();
                                        move |_| Label::new(model.clone()).into_any_element()
                                    },
                                    {
                                        let this = this.clone();
                                        move |cx| {
                                            _ = this.update(cx, |this, cx| {
                                                this.model = model.clone();
                                                cx.notify();
                                            });
                                        }
                                    },
                                );
                            }
                            menu
                        })
                        .into()
                    })
                    .trigger(
                        ButtonLike::new("active-model")
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .overflow_x_hidden()
                                            .flex_grow()
                                            .whitespace_nowrap()
                                            .child(Label::new(self.model.clone())),
                                    )
                                    .child(div().child(
                                        Icon::new(IconName::ChevronDown).color(Color::Muted),
                                    )),
                            )
                            .style(ButtonStyle::Subtle)
                            .tooltip(move |cx| Tooltip::text("Change Model", cx)),
                    )
                    .anchor(gpui::AnchorCorner::TopRight),
            ),
        )
    }
}

impl Render for AssistantChat {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let message_2 = r#"To add a new item to the left side of the titlebar in your Rust code, you need to modify the section of the `CollabTitlebarItem`'s `render` method where you have the comment `// left side`. This section constructs the left side of the titlebar using a chain of `.child()` and `.children()` method calls to add elements horizontally.

        Here is a simplified step-by-step approach:

        1. **Decide on the Item to Add:** First, determine what item you want to add to the left side. For the sake of example, let's say you want to add a simple button.

        2. **Create the New Item:** Based on the example, you'll want to create a new `Button` element, similar to how other UI elements are created in your existing code. Refer to other button creations in your code for examples on how to configure its properties, like label, style, or actions upon clicks.

        3. **Insert the New Item in the Render Method:** Locate the section in the `render` method of `CollabTitlebarItem` where the left side elements are added. This is the section that starts with `TitleBar::new("collab-titlebar")` and adds children to it. You can directly insert the new item within the `h_flex()` block for the left side.

        Assuming you are adding a new button named "NewButton" to the left side of the titlebar, here's an illustrative example showing where and how you might add it:

        ```rust
        impl Render for CollabTitlebarItem {
            fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
                // Your existing code up to the left side modifications
                TitleBar::new("collab-titlebar")
                    .when(cfg!(not(windows)), |this| {
                        // Existing double-click to zoom functionality
                    })
                    // Modify the left side starting here
                    .child(
                        h_flex()
                            .gap_1() // Keep the existing gap
                            // Add your new button here
                            .child(
                                Button::new("new_button", "New Button")
                                    // You can chain any configuration methods you need
                                    .style(ButtonStyle::Subtle)
                                    .on_click(|_, cx| {
                                        // Define click behavior
                                    }),
                            )
                            .children(self.render_project_host(cx)) // Keep existing items
                            .child(self.render_project_name(cx))
                            .children(self.render_project_branch(cx)),
                    )
                    // Your existing code continues from here...
                    .child(...)
                    // The rest of the existing render method...
            }
        }
        ```

        In this example, we added a new button with the label "New Button" to the left side of the titlebar. You can replace the button creation with any other UI element or components you wish to add, and configure it according to your needs. Adjust the `.on_click` handler to define what should happen when the user clicks the new item."#.to_string();

        let messages_vec = vec![
            "How can I add a new item to the left side of the titlebar?".to_string(),
            message_2.to_string(),
        ];

        let composer = cx.new_view(|cx| UIComposer::new(cx));

        v_flex()
            .id("assistant-chat")
            .relative()
            .flex_1()
            .justify_end()
            .overflow_y_scroll()
            .px_5()
            .py_2p5()
            .key_context("AssistantChat")
            .text_color(Color::Default.color(cx))
            .bg(cx.theme().colors().surface_background)
            // .child(self.render_model_dropdown(cx))
            // .child(list(self.list_state.clone()).flex_1())
            .children(messages_vec.iter().map(|message| {
                cx.new_view(|cx| {
                    let rich_message =
                        RichText::new(message.clone(), &[], &self.language_registry.clone());

                    UIChatMessage::new(ChatRole::User, rich_message)
                })
            }))
            .child(composer)
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct MessageId(usize);

impl MessageId {
    fn post_inc(&mut self) -> Self {
        let id = *self;
        self.0 += 1;
        id
    }
}

enum ChatMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
}

impl ChatMessage {
    fn focus_handle(&self, cx: &WindowContext) -> Option<FocusHandle> {
        match self {
            ChatMessage::User(UserMessage { body, .. }) => Some(body.focus_handle(cx)),
            ChatMessage::Assistant(_) => None,
        }
    }
}

struct UserMessage {
    id: MessageId,
    body: View<Editor>,
    contexts: Vec<AssistantContext>,
    _subscription: gpui::Subscription,
}

// chain_of_thought: ... -> search -> search_results -> produce_new_message -> send for the real chat message

struct AssistantMessage {
    id: MessageId,
    body: RichText,
    error: Option<SharedString>,
}

enum AssistantContext {
    Codebase(View<CodebaseContext>),
}

struct CodebaseExcerpt {
    element_id: ElementId,
    path: SharedString,
    text: SharedString,
    score: f32,
    expanded: bool,
}

impl AssistantContext {
    fn render(&self, _cx: &mut ViewContext<AssistantChat>) -> AnyElement {
        match self {
            AssistantContext::Codebase(context) => context.clone().into_any_element(),
        }
    }

    fn completion_messages(&self, cx: &WindowContext) -> Vec<CompletionMessage> {
        match self {
            AssistantContext::Codebase(context) => context.read(cx).completion_messages(),
        }
    }
}

enum CodebaseContext {
    Pending { _task: Task<()> },
    Done(Result<Vec<CodebaseExcerpt>>),
}

impl CodebaseContext {
    fn toggle_expanded(&mut self, element_id: ElementId, cx: &mut ViewContext<Self>) {
        if let CodebaseContext::Done(Ok(excerpts)) = self {
            if let Some(excerpt) = excerpts
                .iter_mut()
                .find(|excerpt| excerpt.element_id == element_id)
            {
                excerpt.expanded = !excerpt.expanded;
                cx.notify();
            }
        }
    }
}

impl Render for CodebaseContext {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        match self {
            CodebaseContext::Pending { .. } => div()
                .h_flex()
                .items_center()
                .gap_1()
                .child(Icon::new(IconName::Ai).color(Color::Muted).into_element())
                .child("Searching codebase..."),
            CodebaseContext::Done(Ok(excerpts)) => {
                div()
                    .v_flex()
                    .gap_2()
                    .children(excerpts.iter().map(|excerpt| {
                        let expanded = excerpt.expanded;
                        let element_id = excerpt.element_id.clone();

                        CollapsibleContainer::new(element_id.clone(), expanded.clone())
                            .start_slot(
                                h_flex()
                                    .gap_1()
                                    .child(Icon::new(IconName::File).color(Color::Muted))
                                    .child(Label::new(excerpt.path.clone()).color(Color::Muted)),
                            )
                            .on_click(cx.listener(move |this, _, cx| {
                                dbg!("listener callback fired");
                                this.toggle_expanded(element_id.clone(), cx);
                            }))
                            .child(
                                div()
                                    .p_2()
                                    .rounded_md()
                                    .bg(cx.theme().colors().editor_background)
                                    .child(
                                        excerpt.text.clone(), // todo!(): Show as an editor block
                                    ),
                            )
                    }))
            }
            CodebaseContext::Done(Err(error)) => div().child(error.to_string()), // todo!,
        }
    }
}

impl CodebaseContext {
    fn new(
        query: impl 'static + Future<Output = Result<String>>,
        populated: oneshot::Sender<bool>,
        project_index: Model<ProjectIndex>,
        fs: Arc<dyn Fs>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let query = query.boxed_local();
        let _task = cx.spawn(|this, mut cx| async move {
            let result = async {
                let query = query.await?;
                let results = this
                    .update(&mut cx, |_this, cx| {
                        project_index.read(cx).search(&query, 16, cx)
                    })?
                    .await;

                let excerpts = results.into_iter().map(|result| {
                    let abs_path = result
                        .worktree
                        .read_with(&cx, |worktree, _| worktree.abs_path().join(&result.path));
                    let fs = fs.clone();

                    async move {
                        let path = result.path.clone();
                        let text = fs.load(&abs_path?).await?;
                        // todo!("what should we do with stale ranges?");
                        let range = cmp::min(result.range.start, text.len())
                            ..cmp::min(result.range.end, text.len());

                        let text = SharedString::from(text[range].to_string());

                        anyhow::Ok(CodebaseExcerpt {
                            element_id: ElementId::Name(nanoid::nanoid!().into()),
                            path: path.to_string_lossy().to_string().into(),
                            text,
                            score: result.score,
                            expanded: false,
                        })
                    }
                });

                anyhow::Ok(
                    futures::future::join_all(excerpts)
                        .await
                        .into_iter()
                        .filter_map(|result| result.log_err())
                        .collect(),
                )
            }
            .await;

            this.update(&mut cx, |this, cx| {
                this.populate(result, populated, cx);
            })
            .ok();
        });

        Self::Pending { _task }
    }

    fn populate(
        &mut self,
        result: Result<Vec<CodebaseExcerpt>>,
        populated: oneshot::Sender<bool>,
        cx: &mut ViewContext<Self>,
    ) {
        let success = result.is_ok();
        *self = Self::Done(result);
        populated.send(success).ok();
        cx.notify();
    }

    fn completion_messages(&self) -> Vec<CompletionMessage> {
        // One system message for the whole batch of excerpts:

        // Semantic search results for user query:
        //
        // Excerpt from $path:
        // ~~~
        // `text`
        // ~~~
        //
        // Excerpt from $path:

        match self {
            CodebaseContext::Done(Ok(excerpts)) => {
                if excerpts.is_empty() {
                    return Vec::new();
                }

                let mut body = "Semantic search reasults for user query:\n".to_string();

                for excerpt in excerpts {
                    body.push_str("Excerpt from ");
                    body.push_str(excerpt.path.as_ref());
                    body.push_str(", score ");
                    body.push_str(&excerpt.score.to_string());
                    body.push_str(":\n");
                    body.push_str("~~~\n");
                    body.push_str(excerpt.text.as_ref());
                    body.push_str("~~~\n");
                }

                vec![CompletionMessage {
                    role: CompletionRole::System,
                    body,
                }]
            }
            _ => vec![],
        }
    }
}

struct UIChatAction {}

enum ChatRole {
    User,
    Assistant,
    Action,
}

struct UIChatMessage {
    id: ElementId,
    role: ChatRole,
    content: RichText,
}

impl UIChatMessage {
    fn new(role: ChatRole, content: RichText) -> Self {
        Self {
            id: ElementId::Name(nanoid::nanoid!().into()),
            role,
            content,
        }
    }
}

impl Render for UIChatMessage {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        div().child(div().h_3().w_full()).child(
            v_flex()
                .group("")
                .bg(hsla(1.0, 1.0, 1.0, 0.06))
                .rounded_md()
                .px_2p5()
                .py_1p5()
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            div()
                                .mb_0p5()
                                .child(Label::new("You").color(Color::Default)),
                        )
                        .child(
                            h_flex()
                                .mr_1()
                                .child(
                                    h_flex().visible_on_hover("").gap_1().child(
                                        IconButton::new("copy_text", IconName::Copy)
                                            .icon_size(IconSize::Small)
                                            .icon_color(Color::Muted),
                                    ),
                                )
                                .child(
                                    IconButton::new("menu", IconName::Ellipsis)
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted),
                                ),
                        ),
                )
                .child(
                    div()
                        // .on_action(cx.listener(Self::submit))
                        .mx_1()
                        .mb_1()
                        .p_1()
                        .rounded_md()
                        .text_color(cx.theme().colors().editor_foreground)
                        .font(ThemeSettings::get_global(cx).buffer_font.clone())
                        .hover(|this| this.bg(cx.theme().colors().editor_background)) // .child(body.read(cx).set_read_only(true))
                        .child(self.content.element(self.id.clone(), cx)),
                ),
        )
    }
}

struct UIComposer {
    editor: View<Editor>,
}

impl UIComposer {
    fn new(cx: &mut ViewContext<Self>) -> Self {
        let editor = cx.new_view(|cx| {
            let mut editor = Editor::auto_height(80, cx);
            editor.set_soft_wrap_mode(SoftWrap::EditorWidth, cx);
            editor.set_placeholder_text("Send a message...", cx);

            editor
        });

        Self { editor }
    }
}

impl Render for UIComposer {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .child(div().h_3().w_full())
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .justify_start()
                    .child(
                        div()
                            .pt_0p5()
                            .child(
                            Avatar::new(
                                "https://encrypted-tbn0.gstatic.com/images?q=tbn:ANd9GcRni2qBxMYI5UwXBrhb5Kds8BamU5DSwjI1bHvhr3S_Pq58Lp4M".to_string()
                            ).size(rems(1.25))
                        ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .w_full()
                            .group("")
                            .bg(cx.theme().colors().editor_background)
                            .rounded_md()
                            .p_2()
                            .child(
                                div()
                                    // .on_action(cx.listener(Self::submit))
                                    .text_color(cx.theme().colors().editor_foreground)
                                    .font(ThemeSettings::get_global(cx).ui_font.clone())
                                    .child(self.editor.clone())
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        h_flex().gap_2().child("model_name")
                                    )
                                    .child(
                                        h_flex()
                                            .mr_1()
                                            .child(
                                                h_flex().visible_on_hover("").gap_1().child(
                                                    IconButton::new(
                                                        "copy_text",
                                                        IconName::Copy,
                                                    )
                                                    .icon_size(IconSize::Small)
                                                    .icon_color(Color::Muted),
                                                ),
                                            )
                                            .child(
                                                Button::new("send_message", "Send")
                                                    .icon(IconName::Return).icon_color(Color::Muted).icon_position(IconPosition::Start).style(ButtonStyle::Filled)
                                            ),
                                    ),
                            )
                    ),
            )
    }
}

// ChatAction -> impl RenderOnce
// ChatMessage -> impl Render
// Composer -> impl Render
