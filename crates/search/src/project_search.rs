use crate::{Direction, SearchOption, SelectMatch, ToggleSearchOption};
use editor::{Anchor, Autoscroll, Editor, MultiBuffer, SelectAll};
use gpui::{
    action, elements::*, keymap::Binding, platform::CursorStyle, AppContext, ElementBox, Entity,
    ModelContext, ModelHandle, MutableAppContext, RenderContext, Task, View, ViewContext,
    ViewHandle,
};
use postage::watch;
use project::{search::SearchQuery, Project};
use std::{
    any::{Any, TypeId},
    cmp::{self, Ordering},
    ops::Range,
    path::PathBuf,
};
use util::ResultExt as _;
use workspace::{Item, ItemHandle, ItemNavHistory, ItemView, Settings, Workspace};

action!(Deploy);
action!(Search);
action!(SearchInNew);
action!(ToggleFocus);

const MAX_TAB_TITLE_LEN: usize = 24;

pub fn init(cx: &mut MutableAppContext) {
    cx.add_bindings([
        Binding::new("cmd-shift-F", ToggleFocus, Some("ProjectSearchView")),
        Binding::new("cmd-f", ToggleFocus, Some("ProjectSearchView")),
        Binding::new("cmd-shift-F", Deploy, Some("Workspace")),
        Binding::new("enter", Search, Some("ProjectSearchView")),
        Binding::new("cmd-enter", SearchInNew, Some("ProjectSearchView")),
        Binding::new(
            "cmd-g",
            SelectMatch(Direction::Next),
            Some("ProjectSearchView"),
        ),
        Binding::new(
            "cmd-shift-G",
            SelectMatch(Direction::Prev),
            Some("ProjectSearchView"),
        ),
    ]);
    cx.add_action(ProjectSearchView::deploy);
    cx.add_action(ProjectSearchView::search);
    cx.add_action(ProjectSearchView::search_in_new);
    cx.add_action(ProjectSearchView::toggle_search_option);
    cx.add_action(ProjectSearchView::select_match);
    cx.add_action(ProjectSearchView::toggle_focus);
    cx.capture_action(ProjectSearchView::tab);
}

struct ProjectSearch {
    project: ModelHandle<Project>,
    excerpts: ModelHandle<MultiBuffer>,
    pending_search: Option<Task<Option<()>>>,
    match_ranges: Vec<Range<Anchor>>,
    active_query: Option<SearchQuery>,
}

struct ProjectSearchView {
    model: ModelHandle<ProjectSearch>,
    query_editor: ViewHandle<Editor>,
    results_editor: ViewHandle<Editor>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    query_contains_error: bool,
    active_match_index: Option<usize>,
    settings: watch::Receiver<Settings>,
}

impl Entity for ProjectSearch {
    type Event = ();
}

impl ProjectSearch {
    fn new(project: ModelHandle<Project>, cx: &mut ModelContext<Self>) -> Self {
        let replica_id = project.read(cx).replica_id();
        Self {
            project,
            excerpts: cx.add_model(|_| MultiBuffer::new(replica_id)),
            pending_search: Default::default(),
            match_ranges: Default::default(),
            active_query: None,
        }
    }

    fn clone(&self, cx: &mut ModelContext<Self>) -> ModelHandle<Self> {
        cx.add_model(|cx| Self {
            project: self.project.clone(),
            excerpts: self
                .excerpts
                .update(cx, |excerpts, cx| cx.add_model(|cx| excerpts.clone(cx))),
            pending_search: Default::default(),
            match_ranges: self.match_ranges.clone(),
            active_query: self.active_query.clone(),
        })
    }

    fn search(&mut self, query: SearchQuery, cx: &mut ModelContext<Self>) {
        let search = self
            .project
            .update(cx, |project, cx| project.search(query.clone(), cx));
        self.active_query = Some(query);
        self.match_ranges.clear();
        self.pending_search = Some(cx.spawn_weak(|this, mut cx| async move {
            let matches = search.await.log_err()?;
            if let Some(this) = this.upgrade(&cx) {
                this.update(&mut cx, |this, cx| {
                    this.match_ranges.clear();
                    let mut matches = matches.into_iter().collect::<Vec<_>>();
                    matches
                        .sort_by_key(|(buffer, _)| buffer.read(cx).file().map(|file| file.path()));
                    this.excerpts.update(cx, |excerpts, cx| {
                        excerpts.clear(cx);
                        for (buffer, buffer_matches) in matches {
                            let ranges_to_highlight = excerpts.push_excerpts_with_context_lines(
                                buffer,
                                buffer_matches.clone(),
                                1,
                                cx,
                            );
                            this.match_ranges.extend(ranges_to_highlight);
                        }
                    });
                    this.pending_search.take();
                    cx.notify();
                });
            }
            None
        }));
        cx.notify();
    }
}

impl Item for ProjectSearch {
    type View = ProjectSearchView;

    fn build_view(
        model: ModelHandle<Self>,
        workspace: &Workspace,
        nav_history: ItemNavHistory,
        cx: &mut gpui::ViewContext<Self::View>,
    ) -> Self::View {
        ProjectSearchView::new(model, nav_history, workspace.settings(), cx)
    }

    fn project_path(&self) -> Option<project::ProjectPath> {
        None
    }
}

enum ViewEvent {
    UpdateTab,
}

impl Entity for ProjectSearchView {
    type Event = ViewEvent;
}

impl View for ProjectSearchView {
    fn ui_name() -> &'static str {
        "ProjectSearchView"
    }

    fn render(&mut self, cx: &mut RenderContext<Self>) -> ElementBox {
        let model = &self.model.read(cx);
        let results = if model.match_ranges.is_empty() {
            let theme = &self.settings.borrow().theme;
            let text = if self.query_editor.read(cx).text(cx).is_empty() {
                ""
            } else if model.pending_search.is_some() {
                "Searching..."
            } else {
                "No results"
            };
            Label::new(text.to_string(), theme.search.results_status.clone())
                .aligned()
                .contained()
                .with_background_color(theme.editor.background)
                .flexible(1., true)
                .boxed()
        } else {
            ChildView::new(&self.results_editor)
                .flexible(1., true)
                .boxed()
        };

        Flex::column()
            .with_child(self.render_query_editor(cx))
            .with_child(results)
            .boxed()
    }

    fn on_focus(&mut self, cx: &mut ViewContext<Self>) {
        if self.model.read(cx).match_ranges.is_empty() {
            cx.focus(&self.query_editor);
        } else {
            self.focus_results_editor(cx);
        }
    }
}

impl ItemView for ProjectSearchView {
    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &ViewHandle<Self>,
        _: &gpui::AppContext,
    ) -> Option<gpui::AnyViewHandle> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.into())
        } else if type_id == TypeId::of::<Editor>() {
            Some((&self.results_editor).into())
        } else {
            None
        }
    }

    fn deactivated(&mut self, cx: &mut ViewContext<Self>) {
        self.results_editor
            .update(cx, |editor, cx| editor.deactivated(cx));
    }

    fn item(&self, _: &gpui::AppContext) -> Box<dyn ItemHandle> {
        Box::new(self.model.clone())
    }

    fn tab_content(&self, tab_theme: &theme::Tab, cx: &gpui::AppContext) -> ElementBox {
        let settings = self.settings.borrow();
        let search_theme = &settings.theme.search;
        Flex::row()
            .with_child(
                Svg::new("icons/magnifier.svg")
                    .with_color(tab_theme.label.text.color)
                    .constrained()
                    .with_width(search_theme.tab_icon_width)
                    .aligned()
                    .boxed(),
            )
            .with_children(self.model.read(cx).active_query.as_ref().map(|query| {
                let query_text = if query.as_str().len() > MAX_TAB_TITLE_LEN {
                    query.as_str()[..MAX_TAB_TITLE_LEN].to_string() + "…"
                } else {
                    query.as_str().to_string()
                };

                Label::new(query_text, tab_theme.label.clone())
                    .aligned()
                    .contained()
                    .with_margin_left(search_theme.tab_icon_spacing)
                    .boxed()
            }))
            .boxed()
    }

    fn project_path(&self, _: &gpui::AppContext) -> Option<project::ProjectPath> {
        None
    }

    fn can_save(&self, _: &gpui::AppContext) -> bool {
        true
    }

    fn is_dirty(&self, cx: &AppContext) -> bool {
        self.results_editor.read(cx).is_dirty(cx)
    }

    fn has_conflict(&self, cx: &AppContext) -> bool {
        self.results_editor.read(cx).has_conflict(cx)
    }

    fn save(
        &mut self,
        project: ModelHandle<Project>,
        cx: &mut ViewContext<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.results_editor
            .update(cx, |editor, cx| editor.save(project, cx))
    }

    fn can_save_as(&self, _: &gpui::AppContext) -> bool {
        false
    }

    fn save_as(
        &mut self,
        _: ModelHandle<Project>,
        _: PathBuf,
        _: &mut ViewContext<Self>,
    ) -> Task<anyhow::Result<()>> {
        unreachable!("save_as should not have been called")
    }

    fn clone_on_split(
        &self,
        nav_history: ItemNavHistory,
        cx: &mut ViewContext<Self>,
    ) -> Option<Self>
    where
        Self: Sized,
    {
        let model = self.model.update(cx, |model, cx| model.clone(cx));
        Some(Self::new(model, nav_history, self.settings.clone(), cx))
    }

    fn navigate(&mut self, data: Box<dyn Any>, cx: &mut ViewContext<Self>) {
        self.results_editor
            .update(cx, |editor, cx| editor.navigate(data, cx));
    }

    fn should_update_tab_on_event(event: &ViewEvent) -> bool {
        matches!(event, ViewEvent::UpdateTab)
    }
}

impl ProjectSearchView {
    fn new(
        model: ModelHandle<ProjectSearch>,
        nav_history: ItemNavHistory,
        settings: watch::Receiver<Settings>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let project;
        let excerpts;
        let mut query_text = String::new();
        let mut regex = false;
        let mut case_sensitive = false;
        let mut whole_word = false;

        {
            let model = model.read(cx);
            project = model.project.clone();
            excerpts = model.excerpts.clone();
            if let Some(active_query) = model.active_query.as_ref() {
                query_text = active_query.as_str().to_string();
                regex = active_query.is_regex();
                case_sensitive = active_query.case_sensitive();
                whole_word = active_query.whole_word();
            }
        }
        cx.observe(&model, |this, _, cx| this.model_changed(true, cx))
            .detach();

        let query_editor = cx.add_view(|cx| {
            let mut editor = Editor::single_line(
                settings.clone(),
                Some(|theme| theme.search.editor.input.clone()),
                cx,
            );
            editor.set_text(query_text, cx);
            editor
        });

        let results_editor = cx.add_view(|cx| {
            let mut editor = Editor::for_buffer(excerpts, Some(project), settings.clone(), cx);
            editor.set_searchable(false);
            editor.set_nav_history(Some(nav_history));
            editor
        });
        cx.observe(&results_editor, |_, _, cx| cx.emit(ViewEvent::UpdateTab))
            .detach();
        cx.subscribe(&results_editor, |this, _, event, cx| {
            if matches!(event, editor::Event::SelectionsChanged) {
                this.update_match_index(cx);
            }
        })
        .detach();

        let mut this = ProjectSearchView {
            model,
            query_editor,
            results_editor,
            case_sensitive,
            whole_word,
            regex,
            query_contains_error: false,
            active_match_index: None,
            settings,
        };
        this.model_changed(false, cx);
        this
    }

    fn deploy(workspace: &mut Workspace, _: &Deploy, cx: &mut ViewContext<Workspace>) {
        if let Some(existing) = workspace
            .items_of_type::<ProjectSearch>(cx)
            .max_by_key(|existing| existing.id())
        {
            workspace.activate_item(&existing, cx);
        } else {
            let model = cx.add_model(|cx| ProjectSearch::new(workspace.project().clone(), cx));
            workspace.open_item(model, cx);
        }
    }

    fn search(&mut self, _: &Search, cx: &mut ViewContext<Self>) {
        if let Some(query) = self.build_search_query(cx) {
            self.model.update(cx, |model, cx| model.search(query, cx));
        }
    }

    fn search_in_new(workspace: &mut Workspace, _: &SearchInNew, cx: &mut ViewContext<Workspace>) {
        if let Some(search_view) = workspace
            .active_item(cx)
            .and_then(|item| item.downcast::<ProjectSearchView>())
        {
            let new_query = search_view.update(cx, |search_view, cx| {
                let new_query = search_view.build_search_query(cx);
                if new_query.is_some() {
                    if let Some(old_query) = search_view.model.read(cx).active_query.clone() {
                        search_view.query_editor.update(cx, |editor, cx| {
                            editor.set_text(old_query.as_str(), cx);
                        });
                        search_view.regex = old_query.is_regex();
                        search_view.whole_word = old_query.whole_word();
                        search_view.case_sensitive = old_query.case_sensitive();
                    }
                }
                new_query
            });
            if let Some(new_query) = new_query {
                let model = cx.add_model(|cx| {
                    let mut model = ProjectSearch::new(workspace.project().clone(), cx);
                    model.search(new_query, cx);
                    model
                });
                workspace.open_item(model, cx);
            }
        }
    }

    fn build_search_query(&mut self, cx: &mut ViewContext<Self>) -> Option<SearchQuery> {
        let text = self.query_editor.read(cx).text(cx);
        if self.regex {
            match SearchQuery::regex(text, self.whole_word, self.case_sensitive) {
                Ok(query) => Some(query),
                Err(_) => {
                    self.query_contains_error = true;
                    cx.notify();
                    None
                }
            }
        } else {
            Some(SearchQuery::text(
                text,
                self.whole_word,
                self.case_sensitive,
            ))
        }
    }

    fn toggle_search_option(
        &mut self,
        ToggleSearchOption(option): &ToggleSearchOption,
        cx: &mut ViewContext<Self>,
    ) {
        let value = match option {
            SearchOption::WholeWord => &mut self.whole_word,
            SearchOption::CaseSensitive => &mut self.case_sensitive,
            SearchOption::Regex => &mut self.regex,
        };
        *value = !*value;
        self.search(&Search, cx);
        cx.notify();
    }

    fn select_match(&mut self, &SelectMatch(direction): &SelectMatch, cx: &mut ViewContext<Self>) {
        if let Some(mut index) = self.active_match_index {
            let range_to_select = {
                let model = self.model.read(cx);
                let results_editor = self.results_editor.read(cx);
                let buffer = results_editor.buffer().read(cx).read(cx);
                let cursor = results_editor.newest_anchor_selection().head();
                let ranges = &model.match_ranges;

                if ranges[index].start.cmp(&cursor, &buffer).unwrap().is_gt() {
                    if direction == Direction::Prev {
                        if index == 0 {
                            index = ranges.len() - 1;
                        } else {
                            index -= 1;
                        }
                    }
                } else if ranges[index].end.cmp(&cursor, &buffer).unwrap().is_lt() {
                    if direction == Direction::Next {
                        index = 0;
                    }
                } else if direction == Direction::Prev {
                    if index == 0 {
                        index = ranges.len() - 1;
                    } else {
                        index -= 1;
                    }
                } else if direction == Direction::Next {
                    if index == ranges.len() - 1 {
                        index = 0
                    } else {
                        index += 1;
                    }
                };
                ranges[index].clone()
            };

            self.results_editor.update(cx, |editor, cx| {
                editor.select_ranges([range_to_select], Some(Autoscroll::Fit), cx);
            });
        }
    }

    fn toggle_focus(&mut self, _: &ToggleFocus, cx: &mut ViewContext<Self>) {
        if self.query_editor.is_focused(cx) {
            if !self.model.read(cx).match_ranges.is_empty() {
                self.focus_results_editor(cx);
            }
        } else {
            self.query_editor.update(cx, |query_editor, cx| {
                query_editor.select_all(&SelectAll, cx);
            });
            cx.focus(&self.query_editor);
        }
    }

    fn tab(&mut self, _: &editor::Tab, cx: &mut ViewContext<Self>) {
        if self.query_editor.is_focused(cx) {
            if !self.model.read(cx).match_ranges.is_empty() {
                self.focus_results_editor(cx);
            }
        } else {
            cx.propagate_action()
        }
    }

    fn focus_results_editor(&self, cx: &mut ViewContext<Self>) {
        self.query_editor.update(cx, |query_editor, cx| {
            let cursor = query_editor.newest_anchor_selection().head();
            query_editor.select_ranges([cursor.clone()..cursor], None, cx);
        });
        cx.focus(&self.results_editor);
    }

    fn model_changed(&mut self, reset_selections: bool, cx: &mut ViewContext<Self>) {
        let match_ranges = self.model.read(cx).match_ranges.clone();
        if match_ranges.is_empty() {
            self.active_match_index = None;
        } else {
            let theme = &self.settings.borrow().theme.search;
            self.results_editor.update(cx, |editor, cx| {
                if reset_selections {
                    editor.select_ranges(match_ranges.first().cloned(), Some(Autoscroll::Fit), cx);
                }
                editor.highlight_ranges::<Self>(match_ranges, theme.match_background, cx);
            });
            if self.query_editor.is_focused(cx) {
                self.focus_results_editor(cx);
            }
        }

        cx.emit(ViewEvent::UpdateTab);
        cx.notify();
    }

    fn update_match_index(&mut self, cx: &mut ViewContext<Self>) {
        let match_ranges = self.model.read(cx).match_ranges.clone();
        if match_ranges.is_empty() {
            self.active_match_index = None;
        } else {
            let results_editor = &self.results_editor.read(cx);
            let cursor = results_editor.newest_anchor_selection().head();
            let new_index = {
                let buffer = results_editor.buffer().read(cx).read(cx);
                match match_ranges.binary_search_by(|probe| {
                    if probe.end.cmp(&cursor, &*buffer).unwrap().is_lt() {
                        Ordering::Less
                    } else if probe.start.cmp(&cursor, &*buffer).unwrap().is_gt() {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                }) {
                    Ok(i) | Err(i) => Some(cmp::min(i, match_ranges.len() - 1)),
                }
            };
            if self.active_match_index != new_index {
                self.active_match_index = new_index;
                cx.notify();
            }
        }
    }

    fn render_query_editor(&self, cx: &mut RenderContext<Self>) -> ElementBox {
        let theme = &self.settings.borrow().theme;
        let editor_container = if self.query_contains_error {
            theme.search.invalid_editor
        } else {
            theme.search.editor.input.container
        };
        Flex::row()
            .with_child(
                ChildView::new(&self.query_editor)
                    .contained()
                    .with_style(editor_container)
                    .aligned()
                    .constrained()
                    .with_max_width(theme.search.editor.max_width)
                    .boxed(),
            )
            .with_child(
                Flex::row()
                    .with_child(self.render_option_button("Case", SearchOption::CaseSensitive, cx))
                    .with_child(self.render_option_button("Word", SearchOption::WholeWord, cx))
                    .with_child(self.render_option_button("Regex", SearchOption::Regex, cx))
                    .contained()
                    .with_style(theme.search.option_button_group)
                    .aligned()
                    .boxed(),
            )
            .with_children({
                self.active_match_index.into_iter().flat_map(|match_ix| {
                    [
                        Flex::row()
                            .with_child(self.render_nav_button("<", Direction::Prev, cx))
                            .with_child(self.render_nav_button(">", Direction::Next, cx))
                            .aligned()
                            .boxed(),
                        Label::new(
                            format!(
                                "{}/{}",
                                match_ix + 1,
                                self.model.read(cx).match_ranges.len()
                            ),
                            theme.search.match_index.text.clone(),
                        )
                        .contained()
                        .with_style(theme.search.match_index.container)
                        .aligned()
                        .boxed(),
                    ]
                })
            })
            .contained()
            .with_style(theme.search.container)
            .constrained()
            .with_height(theme.workspace.toolbar.height)
            .named("project search")
    }

    fn render_option_button(
        &self,
        icon: &str,
        option: SearchOption,
        cx: &mut RenderContext<Self>,
    ) -> ElementBox {
        let theme = &self.settings.borrow().theme.search;
        let is_active = self.is_option_enabled(option);
        MouseEventHandler::new::<Self, _, _>(option as usize, cx, |state, _| {
            let style = match (is_active, state.hovered) {
                (false, false) => &theme.option_button,
                (false, true) => &theme.hovered_option_button,
                (true, false) => &theme.active_option_button,
                (true, true) => &theme.active_hovered_option_button,
            };
            Label::new(icon.to_string(), style.text.clone())
                .contained()
                .with_style(style.container)
                .boxed()
        })
        .on_click(move |cx| cx.dispatch_action(ToggleSearchOption(option)))
        .with_cursor_style(CursorStyle::PointingHand)
        .boxed()
    }

    fn is_option_enabled(&self, option: SearchOption) -> bool {
        match option {
            SearchOption::WholeWord => self.whole_word,
            SearchOption::CaseSensitive => self.case_sensitive,
            SearchOption::Regex => self.regex,
        }
    }

    fn render_nav_button(
        &self,
        icon: &str,
        direction: Direction,
        cx: &mut RenderContext<Self>,
    ) -> ElementBox {
        let theme = &self.settings.borrow().theme.search;
        enum NavButton {}
        MouseEventHandler::new::<NavButton, _, _>(direction as usize, cx, |state, _| {
            let style = if state.hovered {
                &theme.hovered_option_button
            } else {
                &theme.option_button
            };
            Label::new(icon.to_string(), style.text.clone())
                .contained()
                .with_style(style.container)
                .boxed()
        })
        .on_click(move |cx| cx.dispatch_action(SelectMatch(direction)))
        .with_cursor_style(CursorStyle::PointingHand)
        .boxed()
    }
}
