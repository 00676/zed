use crate::{
    active_match_index, match_index_for_direction, Direction, SearchOption, SelectMatch,
    ToggleSearchOption,
};
use collections::HashMap;
use editor::{Anchor, Autoscroll, Editor, MultiBuffer, SelectAll};
use gpui::{
    action, elements::*, keymap::Binding, platform::CursorStyle, AppContext, ElementBox, Entity,
    ModelContext, ModelHandle, MutableAppContext, RenderContext, Subscription, Task, View,
    ViewContext, ViewHandle, WeakModelHandle, WeakViewHandle,
};
use project::{search::SearchQuery, Project};
use std::{
    any::{Any, TypeId},
    ops::Range,
    path::PathBuf,
};
use util::ResultExt as _;
use workspace::{
    Item, ItemNavHistory, Pane, Settings, ToolbarItemLocation, ToolbarItemView, Workspace,
};

action!(Deploy);
action!(Search);
action!(SearchInNew);
action!(ToggleFocus);

const MAX_TAB_TITLE_LEN: usize = 24;

#[derive(Default)]
struct ActiveSearches(HashMap<WeakModelHandle<Project>, WeakViewHandle<ProjectSearchView>>);

pub fn init(cx: &mut MutableAppContext) {
    cx.set_global(ActiveSearches::default());
    cx.add_bindings([
        Binding::new("cmd-shift-F", ToggleFocus, Some("Pane")),
        Binding::new("cmd-f", ToggleFocus, Some("Pane")),
        Binding::new("cmd-shift-F", Deploy, Some("Workspace")),
        Binding::new("enter", Search, Some("ProjectSearchBar")),
        Binding::new("cmd-enter", SearchInNew, Some("ProjectSearchBar")),
        Binding::new("cmd-g", SelectMatch(Direction::Next), Some("Pane")),
        Binding::new("cmd-shift-G", SelectMatch(Direction::Prev), Some("Pane")),
    ]);
    cx.add_action(ProjectSearchView::deploy);
    cx.add_action(ProjectSearchBar::search);
    cx.add_action(ProjectSearchBar::search_in_new);
    cx.add_action(ProjectSearchBar::toggle_search_option);
    cx.add_action(ProjectSearchBar::select_match);
    cx.add_action(ProjectSearchBar::toggle_focus);
    cx.capture_action(ProjectSearchBar::tab);
}

struct ProjectSearch {
    project: ModelHandle<Project>,
    excerpts: ModelHandle<MultiBuffer>,
    pending_search: Option<Task<Option<()>>>,
    match_ranges: Vec<Range<Anchor>>,
    active_query: Option<SearchQuery>,
}

pub struct ProjectSearchView {
    model: ModelHandle<ProjectSearch>,
    query_editor: ViewHandle<Editor>,
    results_editor: ViewHandle<Editor>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    query_contains_error: bool,
    active_match_index: Option<usize>,
}

pub struct ProjectSearchBar {
    active_project_search: Option<ViewHandle<ProjectSearchView>>,
    subscription: Option<Subscription>,
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

pub enum ViewEvent {
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
        if model.match_ranges.is_empty() {
            let theme = &cx.global::<Settings>().theme;
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
                .flex(1., true)
                .boxed()
        } else {
            ChildView::new(&self.results_editor).flex(1., true).boxed()
        }
    }

    fn on_focus(&mut self, cx: &mut ViewContext<Self>) {
        let handle = cx.weak_handle();
        cx.update_global(|state: &mut ActiveSearches, cx| {
            state
                .0
                .insert(self.model.read(cx).project.downgrade(), handle)
        });

        if self.model.read(cx).match_ranges.is_empty() {
            cx.focus(&self.query_editor);
        } else {
            self.focus_results_editor(cx);
        }
    }
}

impl Item for ProjectSearchView {
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

    fn tab_content(&self, tab_theme: &theme::Tab, cx: &gpui::AppContext) -> ElementBox {
        let settings = cx.global::<Settings>();
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

    fn project_entry_id(&self, _: &AppContext) -> Option<project::ProjectEntryId> {
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

    fn clone_on_split(&self, cx: &mut ViewContext<Self>) -> Option<Self>
    where
        Self: Sized,
    {
        let model = self.model.update(cx, |model, cx| model.clone(cx));
        Some(Self::new(model, cx))
    }

    fn set_nav_history(&mut self, nav_history: ItemNavHistory, cx: &mut ViewContext<Self>) {
        self.results_editor.update(cx, |editor, _| {
            editor.set_nav_history(Some(nav_history));
        });
    }

    fn navigate(&mut self, data: Box<dyn Any>, cx: &mut ViewContext<Self>) -> bool {
        self.results_editor
            .update(cx, |editor, cx| editor.navigate(data, cx))
    }

    fn should_update_tab_on_event(event: &ViewEvent) -> bool {
        matches!(event, ViewEvent::UpdateTab)
    }
}

impl ProjectSearchView {
    fn new(model: ModelHandle<ProjectSearch>, cx: &mut ViewContext<Self>) -> Self {
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
            let mut editor =
                Editor::single_line(Some(|theme| theme.search.editor.input.clone()), cx);
            editor.set_text(query_text, cx);
            editor
        });

        let results_editor = cx.add_view(|cx| {
            let mut editor = Editor::for_multibuffer(excerpts, Some(project), cx);
            editor.set_searchable(false);
            editor
        });
        cx.observe(&results_editor, |_, _, cx| cx.emit(ViewEvent::UpdateTab))
            .detach();
        cx.subscribe(&results_editor, |this, _, event, cx| {
            if matches!(event, editor::Event::SelectionsChanged { .. }) {
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
        };
        this.model_changed(false, cx);
        this
    }

    // Re-activate the most recently activated search or the most recent if it has been closed.
    // If no search exists in the workspace, create a new one.
    fn deploy(workspace: &mut Workspace, _: &Deploy, cx: &mut ViewContext<Workspace>) {
        // Clean up entries for dropped projects
        cx.update_global(|state: &mut ActiveSearches, cx| {
            state.0.retain(|project, _| project.is_upgradable(cx))
        });

        let active_search = cx
            .global::<ActiveSearches>()
            .0
            .get(&workspace.project().downgrade());

        let existing = active_search
            .and_then(|active_search| {
                workspace
                    .items_of_type::<ProjectSearchView>(cx)
                    .find(|search| search == active_search)
            })
            .or_else(|| workspace.item_of_type::<ProjectSearchView>(cx));

        if let Some(existing) = existing {
            workspace.activate_item(&existing, cx);
        } else {
            let model = cx.add_model(|cx| ProjectSearch::new(workspace.project().clone(), cx));
            workspace.add_item(
                Box::new(cx.add_view(|cx| ProjectSearchView::new(model, cx))),
                cx,
            );
        }
    }

    fn search(&mut self, cx: &mut ViewContext<Self>) {
        if let Some(query) = self.build_search_query(cx) {
            self.model.update(cx, |model, cx| model.search(query, cx));
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

    fn select_match(&mut self, direction: Direction, cx: &mut ViewContext<Self>) {
        if let Some(index) = self.active_match_index {
            let model = self.model.read(cx);
            let results_editor = self.results_editor.read(cx);
            let new_index = match_index_for_direction(
                &model.match_ranges,
                &results_editor.newest_anchor_selection().head(),
                index,
                direction,
                &results_editor.buffer().read(cx).read(cx),
            );
            let range_to_select = model.match_ranges[new_index].clone();
            self.results_editor.update(cx, |editor, cx| {
                editor.unfold_ranges([range_to_select.clone()], false, cx);
                editor.select_ranges([range_to_select], Some(Autoscroll::Fit), cx);
            });
        }
    }

    fn focus_query_editor(&self, cx: &mut ViewContext<Self>) {
        self.query_editor.update(cx, |query_editor, cx| {
            query_editor.select_all(&SelectAll, cx);
        });
        cx.focus(&self.query_editor);
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
            self.results_editor.update(cx, |editor, cx| {
                if reset_selections {
                    editor.select_ranges(match_ranges.first().cloned(), Some(Autoscroll::Fit), cx);
                }
                let theme = &cx.global::<Settings>().theme.search;
                editor.highlight_background::<Self>(match_ranges, theme.match_background, cx);
            });
            if self.query_editor.is_focused(cx) {
                self.focus_results_editor(cx);
            }
        }

        cx.emit(ViewEvent::UpdateTab);
        cx.notify();
    }

    fn update_match_index(&mut self, cx: &mut ViewContext<Self>) {
        let results_editor = self.results_editor.read(cx);
        let new_index = active_match_index(
            &self.model.read(cx).match_ranges,
            &results_editor.newest_anchor_selection().head(),
            &results_editor.buffer().read(cx).read(cx),
        );
        if self.active_match_index != new_index {
            self.active_match_index = new_index;
            cx.notify();
        }
    }
}

impl ProjectSearchBar {
    pub fn new() -> Self {
        Self {
            active_project_search: Default::default(),
            subscription: Default::default(),
        }
    }

    fn search(&mut self, _: &Search, cx: &mut ViewContext<Self>) {
        if let Some(search_view) = self.active_project_search.as_ref() {
            search_view.update(cx, |search_view, cx| search_view.search(cx));
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
                workspace.add_item(
                    Box::new(cx.add_view(|cx| ProjectSearchView::new(model, cx))),
                    cx,
                );
            }
        }
    }

    fn select_match(
        pane: &mut Pane,
        &SelectMatch(direction): &SelectMatch,
        cx: &mut ViewContext<Pane>,
    ) {
        if let Some(search_view) = pane
            .active_item()
            .and_then(|item| item.downcast::<ProjectSearchView>())
        {
            search_view.update(cx, |search_view, cx| {
                search_view.select_match(direction, cx);
            });
        } else {
            cx.propagate_action();
        }
    }

    fn toggle_focus(pane: &mut Pane, _: &ToggleFocus, cx: &mut ViewContext<Pane>) {
        if let Some(search_view) = pane
            .active_item()
            .and_then(|item| item.downcast::<ProjectSearchView>())
        {
            search_view.update(cx, |search_view, cx| {
                if search_view.query_editor.is_focused(cx) {
                    if !search_view.model.read(cx).match_ranges.is_empty() {
                        search_view.focus_results_editor(cx);
                    }
                } else {
                    search_view.focus_query_editor(cx);
                }
            });
        } else {
            cx.propagate_action();
        }
    }

    fn tab(&mut self, _: &editor::Tab, cx: &mut ViewContext<Self>) {
        if let Some(search_view) = self.active_project_search.as_ref() {
            search_view.update(cx, |search_view, cx| {
                if search_view.query_editor.is_focused(cx) {
                    if !search_view.model.read(cx).match_ranges.is_empty() {
                        search_view.focus_results_editor(cx);
                    }
                } else {
                    cx.propagate_action();
                }
            });
        } else {
            cx.propagate_action();
        }
    }

    fn toggle_search_option(
        &mut self,
        ToggleSearchOption(option): &ToggleSearchOption,
        cx: &mut ViewContext<Self>,
    ) {
        if let Some(search_view) = self.active_project_search.as_ref() {
            search_view.update(cx, |search_view, cx| {
                let value = match option {
                    SearchOption::WholeWord => &mut search_view.whole_word,
                    SearchOption::CaseSensitive => &mut search_view.case_sensitive,
                    SearchOption::Regex => &mut search_view.regex,
                };
                *value = !*value;
                search_view.search(cx);
            });
            cx.notify();
        }
    }

    fn render_nav_button(
        &self,
        icon: &str,
        direction: Direction,
        cx: &mut RenderContext<Self>,
    ) -> ElementBox {
        enum NavButton {}
        MouseEventHandler::new::<NavButton, _, _>(direction as usize, cx, |state, cx| {
            let theme = &cx.global::<Settings>().theme.search;
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

    fn render_option_button(
        &self,
        icon: &str,
        option: SearchOption,
        cx: &mut RenderContext<Self>,
    ) -> ElementBox {
        let is_active = self.is_option_enabled(option, cx);
        MouseEventHandler::new::<ProjectSearchBar, _, _>(option as usize, cx, |state, cx| {
            let theme = &cx.global::<Settings>().theme.search;
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

    fn is_option_enabled(&self, option: SearchOption, cx: &AppContext) -> bool {
        if let Some(search) = self.active_project_search.as_ref() {
            let search = search.read(cx);
            match option {
                SearchOption::WholeWord => search.whole_word,
                SearchOption::CaseSensitive => search.case_sensitive,
                SearchOption::Regex => search.regex,
            }
        } else {
            false
        }
    }
}

impl Entity for ProjectSearchBar {
    type Event = ();
}

impl View for ProjectSearchBar {
    fn ui_name() -> &'static str {
        "ProjectSearchBar"
    }

    fn render(&mut self, cx: &mut RenderContext<Self>) -> ElementBox {
        if let Some(search) = self.active_project_search.as_ref() {
            let search = search.read(cx);
            let theme = cx.global::<Settings>().theme.clone();
            let editor_container = if search.query_contains_error {
                theme.search.invalid_editor
            } else {
                theme.search.editor.input.container
            };
            Flex::row()
                .with_child(
                    Flex::row()
                        .with_child(ChildView::new(&search.query_editor).flex(1., true).boxed())
                        .with_children(search.active_match_index.map(|match_ix| {
                            Label::new(
                                format!(
                                    "{}/{}",
                                    match_ix + 1,
                                    search.model.read(cx).match_ranges.len()
                                ),
                                theme.search.match_index.text.clone(),
                            )
                            .contained()
                            .with_style(theme.search.match_index.container)
                            .aligned()
                            .boxed()
                        }))
                        .contained()
                        .with_style(editor_container)
                        .aligned()
                        .constrained()
                        .with_max_width(theme.search.editor.max_width)
                        .boxed(),
                )
                .with_child(
                    Flex::row()
                        .with_child(self.render_nav_button("<", Direction::Prev, cx))
                        .with_child(self.render_nav_button(">", Direction::Next, cx))
                        .aligned()
                        .boxed(),
                )
                .with_child(
                    Flex::row()
                        .with_child(self.render_option_button(
                            "Case",
                            SearchOption::CaseSensitive,
                            cx,
                        ))
                        .with_child(self.render_option_button("Word", SearchOption::WholeWord, cx))
                        .with_child(self.render_option_button("Regex", SearchOption::Regex, cx))
                        .contained()
                        .with_style(theme.search.option_button_group)
                        .aligned()
                        .boxed(),
                )
                .contained()
                .with_style(theme.workspace.toolbar.container)
                .constrained()
                .with_height(theme.workspace.toolbar.height)
                .named("project search")
        } else {
            Empty::new().boxed()
        }
    }
}

impl ToolbarItemView for ProjectSearchBar {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn workspace::ItemHandle>,
        cx: &mut ViewContext<Self>,
    ) -> ToolbarItemLocation {
        cx.notify();
        self.subscription = None;
        self.active_project_search = None;
        if let Some(search) = active_pane_item.and_then(|i| i.downcast::<ProjectSearchView>()) {
            self.subscription = Some(cx.observe(&search, |_, _, cx| cx.notify()));
            self.active_project_search = Some(search);
            ToolbarItemLocation::PrimaryLeft
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::DisplayPoint;
    use gpui::{color::Color, TestAppContext};
    use project::FakeFs;
    use serde_json::json;
    use std::sync::Arc;

    #[gpui::test]
    async fn test_project_search(cx: &mut TestAppContext) {
        let fonts = cx.font_cache();
        let mut theme = gpui::fonts::with_font_cache(fonts.clone(), || theme::Theme::default());
        theme.search.match_background = Color::red();
        let settings = Settings::new("Courier", &fonts, Arc::new(theme)).unwrap();
        cx.update(|cx| cx.set_global(settings));

        let fs = FakeFs::new(cx.background());
        fs.insert_tree(
            "/dir",
            json!({
                "one.rs": "const ONE: usize = 1;",
                "two.rs": "const TWO: usize = one::ONE + one::ONE;",
                "three.rs": "const THREE: usize = one::ONE + two::TWO;",
                "four.rs": "const FOUR: usize = one::ONE + three::THREE;",
            }),
        )
        .await;
        let project = Project::test(fs.clone(), cx);
        let (tree, _) = project
            .update(cx, |project, cx| {
                project.find_or_create_local_worktree("/dir", true, cx)
            })
            .await
            .unwrap();
        cx.read(|cx| tree.read(cx).as_local().unwrap().scan_complete())
            .await;

        let search = cx.add_model(|cx| ProjectSearch::new(project, cx));
        let search_view = cx.add_view(Default::default(), |cx| {
            ProjectSearchView::new(search.clone(), cx)
        });

        search_view.update(cx, |search_view, cx| {
            search_view
                .query_editor
                .update(cx, |query_editor, cx| query_editor.set_text("TWO", cx));
            search_view.search(cx);
        });
        search_view.next_notification(&cx).await;
        search_view.update(cx, |search_view, cx| {
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.display_text(cx)),
                "\n\nconst THREE: usize = one::ONE + two::TWO;\n\n\nconst TWO: usize = one::ONE + one::ONE;"
            );
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.all_background_highlights(cx)),
                &[
                    (
                        DisplayPoint::new(2, 32)..DisplayPoint::new(2, 35),
                        Color::red()
                    ),
                    (
                        DisplayPoint::new(2, 37)..DisplayPoint::new(2, 40),
                        Color::red()
                    ),
                    (
                        DisplayPoint::new(5, 6)..DisplayPoint::new(5, 9),
                        Color::red()
                    )
                ]
            );
            assert_eq!(search_view.active_match_index, Some(0));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(2, 32)..DisplayPoint::new(2, 35)]
            );

            search_view.select_match(Direction::Next, cx);
        });

        search_view.update(cx, |search_view, cx| {
            assert_eq!(search_view.active_match_index, Some(1));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(2, 37)..DisplayPoint::new(2, 40)]
            );
            search_view.select_match(Direction::Next, cx);
        });

        search_view.update(cx, |search_view, cx| {
            assert_eq!(search_view.active_match_index, Some(2));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(5, 6)..DisplayPoint::new(5, 9)]
            );
            search_view.select_match(Direction::Next, cx);
        });

        search_view.update(cx, |search_view, cx| {
            assert_eq!(search_view.active_match_index, Some(0));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(2, 32)..DisplayPoint::new(2, 35)]
            );
            search_view.select_match(Direction::Prev, cx);
        });

        search_view.update(cx, |search_view, cx| {
            assert_eq!(search_view.active_match_index, Some(2));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(5, 6)..DisplayPoint::new(5, 9)]
            );
            search_view.select_match(Direction::Prev, cx);
        });

        search_view.update(cx, |search_view, cx| {
            assert_eq!(search_view.active_match_index, Some(1));
            assert_eq!(
                search_view
                    .results_editor
                    .update(cx, |editor, cx| editor.selected_display_ranges(cx)),
                [DisplayPoint::new(2, 37)..DisplayPoint::new(2, 40)]
            );
        });
    }
}
