pub mod items;

use anyhow::Result;
use collections::{BTreeSet, HashMap, HashSet};
use editor::{
    diagnostic_block_renderer, diagnostic_style,
    display_map::{BlockDisposition, BlockId, BlockProperties, RenderBlock},
    items::BufferItemHandle,
    Autoscroll, BuildSettings, Editor, ExcerptId, ExcerptProperties, MultiBuffer, ToOffset,
};
use gpui::{
    action, elements::*, keymap::Binding, AppContext, Entity, ModelHandle, MutableAppContext,
    RenderContext, Task, View, ViewContext, ViewHandle, WeakViewHandle,
};
use language::{Bias, Buffer, Diagnostic, DiagnosticEntry, Point, Selection, SelectionGoal};
use postage::watch;
use project::{Project, ProjectPath, WorktreeId};
use std::{cmp::Ordering, mem, ops::Range, sync::Arc};
use util::TryFutureExt;
use workspace::Workspace;

action!(Deploy);
action!(OpenExcerpts);

const CONTEXT_LINE_COUNT: u32 = 1;

pub fn init(cx: &mut MutableAppContext) {
    cx.add_bindings([
        Binding::new("alt-shift-D", Deploy, Some("Workspace")),
        Binding::new(
            "alt-shift-D",
            OpenExcerpts,
            Some("ProjectDiagnosticsEditor"),
        ),
    ]);
    cx.add_action(ProjectDiagnosticsEditor::deploy);
    cx.add_action(ProjectDiagnosticsEditor::open_excerpts);
}

type Event = editor::Event;

struct ProjectDiagnostics {
    project: ModelHandle<Project>,
}

struct ProjectDiagnosticsEditor {
    model: ModelHandle<ProjectDiagnostics>,
    workspace: WeakViewHandle<Workspace>,
    editor: ViewHandle<Editor>,
    excerpts: ModelHandle<MultiBuffer>,
    path_states: Vec<PathState>,
    paths_to_update: HashMap<WorktreeId, BTreeSet<ProjectPath>>,
    build_settings: BuildSettings,
    settings: watch::Receiver<workspace::Settings>,
}

struct PathState {
    path: ProjectPath,
    header: Option<BlockId>,
    diagnostic_groups: Vec<DiagnosticGroupState>,
}

struct DiagnosticGroupState {
    primary_diagnostic: DiagnosticEntry<language::Anchor>,
    primary_excerpt_ix: usize,
    excerpts: Vec<ExcerptId>,
    blocks: HashSet<BlockId>,
    block_count: usize,
}

impl ProjectDiagnostics {
    fn new(project: ModelHandle<Project>) -> Self {
        Self { project }
    }
}

impl Entity for ProjectDiagnostics {
    type Event = ();
}

impl Entity for ProjectDiagnosticsEditor {
    type Event = Event;
}

impl View for ProjectDiagnosticsEditor {
    fn ui_name() -> &'static str {
        "ProjectDiagnosticsEditor"
    }

    fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
        if self.path_states.is_empty() {
            let theme = &self.settings.borrow().theme.project_diagnostics;
            Label::new(
                "No problems detected in the project".to_string(),
                theme.empty_message.clone(),
            )
            .aligned()
            .contained()
            .with_style(theme.container)
            .boxed()
        } else {
            ChildView::new(self.editor.id()).boxed()
        }
    }

    fn on_focus(&mut self, cx: &mut ViewContext<Self>) {
        if !self.path_states.is_empty() {
            cx.focus(&self.editor);
        }
    }
}

impl ProjectDiagnosticsEditor {
    fn new(
        model: ModelHandle<ProjectDiagnostics>,
        workspace: WeakViewHandle<Workspace>,
        settings: watch::Receiver<workspace::Settings>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let project = model.read(cx).project.clone();
        cx.subscribe(&project, |this, _, event, cx| match event {
            project::Event::DiskBasedDiagnosticsUpdated { worktree_id } => {
                if let Some(paths) = this.paths_to_update.remove(&worktree_id) {
                    this.update_excerpts(paths, cx);
                }
            }
            project::Event::DiagnosticsUpdated(path) => {
                this.paths_to_update
                    .entry(path.worktree_id)
                    .or_default()
                    .insert(path.clone());
            }
            _ => {}
        })
        .detach();

        let excerpts = cx.add_model(|cx| MultiBuffer::new(project.read(cx).replica_id()));
        let build_settings = editor::settings_builder(excerpts.downgrade(), settings.clone());
        let editor =
            cx.add_view(|cx| Editor::for_buffer(excerpts.clone(), build_settings.clone(), cx));
        cx.subscribe(&editor, |_, _, event, cx| cx.emit(*event))
            .detach();

        let paths_to_update = project
            .read(cx)
            .diagnostic_summaries(cx)
            .map(|e| e.0)
            .collect();
        let this = Self {
            model,
            workspace,
            excerpts,
            editor,
            build_settings,
            settings,
            path_states: Default::default(),
            paths_to_update: Default::default(),
        };
        this.update_excerpts(paths_to_update, cx);
        this
    }

    #[cfg(test)]
    fn text(&self, cx: &AppContext) -> String {
        self.editor.read(cx).text(cx)
    }

    fn deploy(workspace: &mut Workspace, _: &Deploy, cx: &mut ViewContext<Workspace>) {
        if let Some(existing) = workspace.item_of_type::<ProjectDiagnostics>(cx) {
            workspace.activate_item(&existing, cx);
        } else {
            let diagnostics =
                cx.add_model(|_| ProjectDiagnostics::new(workspace.project().clone()));
            workspace.open_item(diagnostics, cx);
        }
    }

    fn open_excerpts(&mut self, _: &OpenExcerpts, cx: &mut ViewContext<Self>) {
        if let Some(workspace) = self.workspace.upgrade(cx) {
            let editor = self.editor.read(cx);
            let excerpts = self.excerpts.read(cx);
            let mut new_selections_by_buffer = HashMap::default();

            for selection in editor.local_selections::<usize>(cx) {
                for (buffer, mut range) in
                    excerpts.excerpted_buffers(selection.start..selection.end, cx)
                {
                    if selection.reversed {
                        mem::swap(&mut range.start, &mut range.end);
                    }
                    new_selections_by_buffer
                        .entry(buffer)
                        .or_insert(Vec::new())
                        .push(range)
                }
            }

            workspace.update(cx, |workspace, cx| {
                for (buffer, ranges) in new_selections_by_buffer {
                    let buffer = BufferItemHandle(buffer);
                    if !workspace.activate_pane_for_item(&buffer, cx) {
                        workspace.activate_next_pane(cx);
                    }
                    let editor = workspace
                        .open_item(buffer, cx)
                        .to_any()
                        .downcast::<Editor>()
                        .unwrap();
                    editor.update(cx, |editor, cx| {
                        editor.select_ranges(ranges, Some(Autoscroll::Center), cx)
                    });
                }
            });
        }
    }

    fn update_excerpts(&self, paths: BTreeSet<ProjectPath>, cx: &mut ViewContext<Self>) {
        let project = self.model.read(cx).project.clone();
        cx.spawn(|this, mut cx| {
            async move {
                for path in paths {
                    let buffer = project
                        .update(&mut cx, |project, cx| project.open_buffer(path.clone(), cx))
                        .await?;
                    this.update(&mut cx, |view, cx| view.populate_excerpts(path, buffer, cx))
                }
                Result::<_, anyhow::Error>::Ok(())
            }
            .log_err()
        })
        .detach();
    }

    fn populate_excerpts(
        &mut self,
        path: ProjectPath,
        buffer: ModelHandle<Buffer>,
        cx: &mut ViewContext<Self>,
    ) {
        let was_empty = self.path_states.is_empty();
        let snapshot = buffer.read(cx).snapshot();
        let path_ix = match self.path_states.binary_search_by_key(&&path, |e| &e.path) {
            Ok(ix) => ix,
            Err(ix) => {
                self.path_states.insert(
                    ix,
                    PathState {
                        path: path.clone(),
                        header: None,
                        diagnostic_groups: Default::default(),
                    },
                );
                ix
            }
        };

        let mut prev_excerpt_id = if path_ix > 0 {
            let prev_path_last_group = &self.path_states[path_ix - 1]
                .diagnostic_groups
                .last()
                .unwrap();
            prev_path_last_group.excerpts.last().unwrap().clone()
        } else {
            ExcerptId::min()
        };

        let path_state = &mut self.path_states[path_ix];
        let mut groups_to_add = Vec::new();
        let mut group_ixs_to_remove = Vec::new();
        let mut blocks_to_add = Vec::new();
        let mut blocks_to_remove = HashSet::default();
        let mut first_excerpt_id = None;
        let excerpts_snapshot = self.excerpts.update(cx, |excerpts, excerpts_cx| {
            let mut old_groups = path_state.diagnostic_groups.iter().enumerate().peekable();
            let mut new_groups = snapshot
                .diagnostic_groups()
                .into_iter()
                .filter(|group| group.entries[group.primary_ix].diagnostic.is_disk_based)
                .peekable();

            loop {
                let mut to_insert = None;
                let mut to_remove = None;
                let mut to_keep = None;
                match (old_groups.peek(), new_groups.peek()) {
                    (None, None) => break,
                    (None, Some(_)) => to_insert = new_groups.next(),
                    (Some(_), None) => to_remove = old_groups.next(),
                    (Some((_, old_group)), Some(new_group)) => {
                        let old_primary = &old_group.primary_diagnostic;
                        let new_primary = &new_group.entries[new_group.primary_ix];
                        match compare_diagnostics(old_primary, new_primary, &snapshot) {
                            Ordering::Less => to_remove = old_groups.next(),
                            Ordering::Equal => {
                                to_keep = old_groups.next();
                                new_groups.next();
                            }
                            Ordering::Greater => to_insert = new_groups.next(),
                        }
                    }
                }

                if let Some(group) = to_insert {
                    let mut group_state = DiagnosticGroupState {
                        primary_diagnostic: group.entries[group.primary_ix].clone(),
                        primary_excerpt_ix: 0,
                        excerpts: Default::default(),
                        blocks: Default::default(),
                        block_count: 0,
                    };
                    let mut pending_range: Option<(Range<Point>, usize)> = None;
                    let mut is_first_excerpt_for_group = true;
                    for (ix, entry) in group.entries.iter().map(Some).chain([None]).enumerate() {
                        let resolved_entry = entry.map(|e| e.resolve::<Point>(&snapshot));
                        if let Some((range, start_ix)) = &mut pending_range {
                            if let Some(entry) = resolved_entry.as_ref() {
                                if entry.range.start.row
                                    <= range.end.row + 1 + CONTEXT_LINE_COUNT * 2
                                {
                                    range.end = range.end.max(entry.range.end);
                                    continue;
                                }
                            }

                            let excerpt_start =
                                Point::new(range.start.row.saturating_sub(CONTEXT_LINE_COUNT), 0);
                            let excerpt_end = snapshot.clip_point(
                                Point::new(range.end.row + CONTEXT_LINE_COUNT, u32::MAX),
                                Bias::Left,
                            );
                            let excerpt_id = excerpts.insert_excerpt_after(
                                &prev_excerpt_id,
                                ExcerptProperties {
                                    buffer: &buffer,
                                    range: excerpt_start..excerpt_end,
                                },
                                excerpts_cx,
                            );

                            prev_excerpt_id = excerpt_id.clone();
                            first_excerpt_id.get_or_insert_with(|| prev_excerpt_id.clone());
                            group_state.excerpts.push(excerpt_id.clone());
                            let header_position = (excerpt_id.clone(), language::Anchor::min());

                            if is_first_excerpt_for_group {
                                is_first_excerpt_for_group = false;
                                let primary = &group.entries[group.primary_ix].diagnostic;
                                let mut header = primary.clone();
                                header.message =
                                    primary.message.split('\n').next().unwrap().to_string();
                                group_state.block_count += 1;
                                blocks_to_add.push(BlockProperties {
                                    position: header_position,
                                    height: 2,
                                    render: diagnostic_header_renderer(
                                        header,
                                        true,
                                        self.build_settings.clone(),
                                    ),
                                    disposition: BlockDisposition::Above,
                                });
                            } else {
                                group_state.block_count += 1;
                                blocks_to_add.push(BlockProperties {
                                    position: header_position,
                                    height: 1,
                                    render: context_header_renderer(self.build_settings.clone()),
                                    disposition: BlockDisposition::Above,
                                });
                            }

                            for entry in &group.entries[*start_ix..ix] {
                                let mut diagnostic = entry.diagnostic.clone();
                                if diagnostic.is_primary {
                                    group_state.primary_excerpt_ix = group_state.excerpts.len() - 1;
                                    diagnostic.message =
                                        entry.diagnostic.message.split('\n').skip(1).collect();
                                }

                                if !diagnostic.message.is_empty() {
                                    group_state.block_count += 1;
                                    blocks_to_add.push(BlockProperties {
                                        position: (excerpt_id.clone(), entry.range.start.clone()),
                                        height: diagnostic.message.matches('\n').count() as u8 + 1,
                                        render: diagnostic_block_renderer(
                                            diagnostic,
                                            true,
                                            self.build_settings.clone(),
                                        ),
                                        disposition: BlockDisposition::Below,
                                    });
                                }
                            }

                            pending_range.take();
                        }

                        if let Some(entry) = resolved_entry {
                            pending_range = Some((entry.range.clone(), ix));
                        }
                    }

                    groups_to_add.push(group_state);
                } else if let Some((group_ix, group_state)) = to_remove {
                    excerpts.remove_excerpts(group_state.excerpts.iter(), excerpts_cx);
                    group_ixs_to_remove.push(group_ix);
                    blocks_to_remove.extend(group_state.blocks.iter().copied());
                } else if let Some((_, group)) = to_keep {
                    prev_excerpt_id = group.excerpts.last().unwrap().clone();
                    first_excerpt_id.get_or_insert_with(|| prev_excerpt_id.clone());
                }
            }

            excerpts.snapshot(excerpts_cx)
        });

        self.editor.update(cx, |editor, cx| {
            blocks_to_remove.extend(path_state.header);
            editor.remove_blocks(blocks_to_remove, cx);
            let header_block = first_excerpt_id.map(|excerpt_id| BlockProperties {
                position: excerpts_snapshot.anchor_in_excerpt(excerpt_id, language::Anchor::min()),
                height: 2,
                render: path_header_renderer(buffer, self.build_settings.clone()),
                disposition: BlockDisposition::Above,
            });
            let mut block_ids = editor
                .insert_blocks(
                    header_block
                        .into_iter()
                        .chain(blocks_to_add.into_iter().map(|block| {
                            let (excerpt_id, text_anchor) = block.position;
                            BlockProperties {
                                position: excerpts_snapshot
                                    .anchor_in_excerpt(excerpt_id, text_anchor),
                                height: block.height,
                                render: block.render,
                                disposition: block.disposition,
                            }
                        })),
                    cx,
                )
                .into_iter();

            path_state.header = block_ids.next();
            for group_state in &mut groups_to_add {
                group_state.blocks = block_ids.by_ref().take(group_state.block_count).collect();
            }
        });

        for ix in group_ixs_to_remove.into_iter().rev() {
            path_state.diagnostic_groups.remove(ix);
        }
        path_state.diagnostic_groups.extend(groups_to_add);
        path_state.diagnostic_groups.sort_unstable_by(|a, b| {
            let range_a = &a.primary_diagnostic.range;
            let range_b = &b.primary_diagnostic.range;
            range_a
                .start
                .cmp(&range_b.start, &snapshot)
                .unwrap()
                .then_with(|| range_a.end.cmp(&range_b.end, &snapshot).unwrap())
        });

        if path_state.diagnostic_groups.is_empty() {
            self.path_states.remove(path_ix);
        }

        self.editor.update(cx, |editor, cx| {
            let groups;
            let mut selections;
            let new_excerpt_ids_by_selection_id;
            if was_empty {
                groups = self.path_states.first()?.diagnostic_groups.as_slice();
                new_excerpt_ids_by_selection_id = [(0, ExcerptId::min())].into_iter().collect();
                selections = vec![Selection {
                    id: 0,
                    start: 0,
                    end: 0,
                    reversed: false,
                    goal: SelectionGoal::None,
                }];
            } else {
                groups = self.path_states.get(path_ix)?.diagnostic_groups.as_slice();
                new_excerpt_ids_by_selection_id = editor.refresh_selections(cx);
                selections = editor.local_selections::<usize>(cx);
            }

            // If any selection has lost its position, move it to start of the next primary diagnostic.
            for selection in &mut selections {
                if let Some(new_excerpt_id) = new_excerpt_ids_by_selection_id.get(&selection.id) {
                    let group_ix = match groups.binary_search_by(|probe| {
                        probe.excerpts.last().unwrap().cmp(&new_excerpt_id)
                    }) {
                        Ok(ix) | Err(ix) => ix,
                    };
                    if let Some(group) = groups.get(group_ix) {
                        let offset = excerpts_snapshot
                            .anchor_in_excerpt(
                                group.excerpts[group.primary_excerpt_ix].clone(),
                                group.primary_diagnostic.range.start.clone(),
                            )
                            .to_offset(&excerpts_snapshot);
                        selection.start = offset;
                        selection.end = offset;
                    }
                }
            }
            editor.update_selections(selections, None, cx);
            Some(())
        });

        if self.path_states.is_empty() {
            if self.editor.is_focused(cx) {
                cx.focus_self();
            }
        } else {
            if cx.handle().is_focused(cx) {
                cx.focus(&self.editor);
            }
        }
        cx.notify();
    }
}

impl workspace::Item for ProjectDiagnostics {
    type View = ProjectDiagnosticsEditor;

    fn build_view(
        handle: ModelHandle<Self>,
        workspace: &Workspace,
        cx: &mut ViewContext<Self::View>,
    ) -> Self::View {
        ProjectDiagnosticsEditor::new(handle, workspace.weak_handle(), workspace.settings(), cx)
    }

    fn project_path(&self) -> Option<project::ProjectPath> {
        None
    }
}

impl workspace::ItemView for ProjectDiagnosticsEditor {
    type ItemHandle = ModelHandle<ProjectDiagnostics>;

    fn item_handle(&self, _: &AppContext) -> Self::ItemHandle {
        self.model.clone()
    }

    fn title(&self, _: &AppContext) -> String {
        "Project Diagnostics".to_string()
    }

    fn project_path(&self, _: &AppContext) -> Option<project::ProjectPath> {
        None
    }

    fn is_dirty(&self, cx: &AppContext) -> bool {
        self.excerpts.read(cx).read(cx).is_dirty()
    }

    fn has_conflict(&self, cx: &AppContext) -> bool {
        self.excerpts.read(cx).read(cx).has_conflict()
    }

    fn can_save(&self, _: &AppContext) -> bool {
        true
    }

    fn save(&mut self, cx: &mut ViewContext<Self>) -> Result<Task<Result<()>>> {
        self.excerpts.update(cx, |excerpts, cx| excerpts.save(cx))
    }

    fn can_save_as(&self, _: &AppContext) -> bool {
        false
    }

    fn save_as(
        &mut self,
        _: ModelHandle<project::Worktree>,
        _: &std::path::Path,
        _: &mut ViewContext<Self>,
    ) -> Task<Result<()>> {
        unreachable!()
    }

    fn should_activate_item_on_event(event: &Self::Event) -> bool {
        Editor::should_activate_item_on_event(event)
    }

    fn should_update_tab_on_event(event: &Event) -> bool {
        matches!(
            event,
            Event::Saved | Event::Dirtied | Event::FileHandleChanged
        )
    }
}

fn path_header_renderer(buffer: ModelHandle<Buffer>, build_settings: BuildSettings) -> RenderBlock {
    Arc::new(move |cx| {
        let settings = build_settings(cx);
        let file_path = if let Some(file) = buffer.read(&**cx).file() {
            file.path().to_string_lossy().to_string()
        } else {
            "untitled".to_string()
        };
        let mut text_style = settings.style.text.clone();
        let style = settings.style.diagnostic_path_header;
        text_style.color = style.text;
        Label::new(file_path, text_style)
            .aligned()
            .left()
            .contained()
            .with_style(style.header)
            .with_padding_left(cx.line_number_x)
            .expanded()
            .boxed()
    })
}

fn diagnostic_header_renderer(
    diagnostic: Diagnostic,
    is_valid: bool,
    build_settings: BuildSettings,
) -> RenderBlock {
    Arc::new(move |cx| {
        let settings = build_settings(cx);
        let mut text_style = settings.style.text.clone();
        let diagnostic_style = diagnostic_style(diagnostic.severity, is_valid, &settings.style);
        text_style.color = diagnostic_style.text;
        Text::new(diagnostic.message.clone(), text_style)
            .with_soft_wrap(false)
            .aligned()
            .left()
            .contained()
            .with_style(diagnostic_style.header)
            .with_padding_left(cx.line_number_x)
            .expanded()
            .boxed()
    })
}

fn context_header_renderer(build_settings: BuildSettings) -> RenderBlock {
    Arc::new(move |cx| {
        let settings = build_settings(cx);
        let text_style = settings.style.text.clone();
        Label::new("…".to_string(), text_style)
            .contained()
            .with_padding_left(cx.line_number_x)
            .boxed()
    })
}

fn compare_diagnostics<L: language::ToOffset, R: language::ToOffset>(
    lhs: &DiagnosticEntry<L>,
    rhs: &DiagnosticEntry<R>,
    snapshot: &language::BufferSnapshot,
) -> Ordering {
    lhs.range
        .start
        .to_offset(&snapshot)
        .cmp(&rhs.range.start.to_offset(snapshot))
        .then_with(|| {
            lhs.range
                .end
                .to_offset(&snapshot)
                .cmp(&rhs.range.end.to_offset(snapshot))
        })
        .then_with(|| lhs.diagnostic.message.cmp(&rhs.diagnostic.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::{http::ServerResponse, test::FakeHttpClient, Client, UserStore};
    use editor::DisplayPoint;
    use gpui::TestAppContext;
    use language::{Diagnostic, DiagnosticEntry, DiagnosticSeverity, LanguageRegistry, PointUtf16};
    use project::{worktree, FakeFs};
    use serde_json::json;
    use std::sync::Arc;
    use unindent::Unindent as _;
    use workspace::WorkspaceParams;

    #[gpui::test]
    async fn test_diagnostics(mut cx: TestAppContext) {
        let workspace_params = cx.update(WorkspaceParams::test);
        let settings = workspace_params.settings.clone();
        let http_client = FakeHttpClient::new(|_| async move { Ok(ServerResponse::new(404)) });
        let client = Client::new(http_client.clone());
        let user_store = cx.add_model(|cx| UserStore::new(client.clone(), http_client, cx));
        let fs = Arc::new(FakeFs::new());

        let project = cx.update(|cx| {
            Project::local(
                client.clone(),
                user_store,
                Arc::new(LanguageRegistry::new()),
                fs.clone(),
                cx,
            )
        });

        fs.insert_tree(
            "/test",
            json!({
                "a.rs": "
                    const a: i32 = 'a';
                ".unindent(),

                "main.rs": "
                    fn main() {
                        let x = vec![];
                        let y = vec![];
                        a(x);
                        b(y);
                        // comment 1
                        // comment 2
                        c(y);
                        d(x);
                    }
                "
                .unindent(),
            }),
        )
        .await;

        let worktree = project
            .update(&mut cx, |project, cx| {
                project.add_local_worktree("/test", cx)
            })
            .await
            .unwrap();

        worktree.update(&mut cx, |worktree, cx| {
            worktree
                .update_diagnostic_entries(
                    Arc::from("/test/main.rs".as_ref()),
                    None,
                    vec![
                        DiagnosticEntry {
                            range: PointUtf16::new(1, 8)..PointUtf16::new(1, 9),
                            diagnostic: Diagnostic {
                                message:
                                    "move occurs because `x` has type `Vec<char>`, which does not implement the `Copy` trait"
                                        .to_string(),
                                severity: DiagnosticSeverity::INFORMATION,
                                is_primary: false,
                                is_disk_based: true,
                                group_id: 1,
                                ..Default::default()
                            },
                        },
                        DiagnosticEntry {
                            range: PointUtf16::new(2, 8)..PointUtf16::new(2, 9),
                            diagnostic: Diagnostic {
                                message:
                                    "move occurs because `y` has type `Vec<char>`, which does not implement the `Copy` trait"
                                        .to_string(),
                                severity: DiagnosticSeverity::INFORMATION,
                                is_primary: false,
                                is_disk_based: true,
                                group_id: 0,
                                ..Default::default()
                            },
                        },
                        DiagnosticEntry {
                            range: PointUtf16::new(3, 6)..PointUtf16::new(3, 7),
                            diagnostic: Diagnostic {
                                message: "value moved here".to_string(),
                                severity: DiagnosticSeverity::INFORMATION,
                                is_primary: false,
                                is_disk_based: true,
                                group_id: 1,
                                ..Default::default()
                            },
                        },
                        DiagnosticEntry {
                            range: PointUtf16::new(4, 6)..PointUtf16::new(4, 7),
                            diagnostic: Diagnostic {
                                message: "value moved here".to_string(),
                                severity: DiagnosticSeverity::INFORMATION,
                                is_primary: false,
                                is_disk_based: true,
                                group_id: 0,
                                ..Default::default()
                            },
                        },
                        DiagnosticEntry {
                            range: PointUtf16::new(7, 6)..PointUtf16::new(7, 7),
                            diagnostic: Diagnostic {
                                message: "use of moved value\nvalue used here after move".to_string(),
                                severity: DiagnosticSeverity::ERROR,
                                is_primary: true,
                                is_disk_based: true,
                                group_id: 0,
                                ..Default::default()
                            },
                        },
                        DiagnosticEntry {
                            range: PointUtf16::new(8, 6)..PointUtf16::new(8, 7),
                            diagnostic: Diagnostic {
                                message: "use of moved value\nvalue used here after move".to_string(),
                                severity: DiagnosticSeverity::ERROR,
                                is_primary: true,
                                is_disk_based: true,
                                group_id: 1,
                                ..Default::default()
                            },
                        },
                    ],
                    cx,
                )
                .unwrap();
        });

        let model = cx.add_model(|_| ProjectDiagnostics::new(project.clone()));
        let workspace = cx.add_view(0, |cx| Workspace::new(&workspace_params, cx));

        let view = cx.add_view(0, |cx| {
            ProjectDiagnosticsEditor::new(model, workspace.downgrade(), settings, cx)
        });

        view.condition(&mut cx, |view, cx| view.text(cx).contains("fn main()"))
            .await;

        view.update(&mut cx, |view, cx| {
            let editor = view.editor.update(cx, |editor, cx| editor.snapshot(cx));

            assert_eq!(
                editor.text(),
                concat!(
                    //
                    // main.rs
                    //
                    "\n", // filename
                    "\n", // padding
                    // diagnostic group 1
                    "\n", // primary message
                    "\n", // padding
                    "    let x = vec![];\n",
                    "    let y = vec![];\n",
                    "\n", // supporting diagnostic
                    "    a(x);\n",
                    "    b(y);\n",
                    "\n", // supporting diagnostic
                    "    // comment 1\n",
                    "    // comment 2\n",
                    "    c(y);\n",
                    "\n", // supporting diagnostic
                    "    d(x);\n",
                    // diagnostic group 2
                    "\n", // primary message
                    "\n", // padding
                    "fn main() {\n",
                    "    let x = vec![];\n",
                    "\n", // supporting diagnostic
                    "    let y = vec![];\n",
                    "    a(x);\n",
                    "\n", // supporting diagnostic
                    "    b(y);\n",
                    "\n", // context ellipsis
                    "    c(y);\n",
                    "    d(x);\n",
                    "\n", // supporting diagnostic
                    "}"
                )
            );

            view.editor.update(cx, |editor, cx| {
                assert_eq!(
                    editor.selected_display_ranges(cx),
                    [DisplayPoint::new(12, 6)..DisplayPoint::new(12, 6)]
                );
            });
        });

        worktree.update(&mut cx, |worktree, cx| {
            worktree
                .update_diagnostic_entries(
                    Arc::from("/test/a.rs".as_ref()),
                    None,
                    vec![DiagnosticEntry {
                        range: PointUtf16::new(0, 15)..PointUtf16::new(0, 15),
                        diagnostic: Diagnostic {
                            message: "mismatched types\nexpected `usize`, found `char`".to_string(),
                            severity: DiagnosticSeverity::ERROR,
                            is_primary: true,
                            is_disk_based: true,
                            group_id: 0,
                            ..Default::default()
                        },
                    }],
                    cx,
                )
                .unwrap();
            cx.emit(worktree::Event::DiskBasedDiagnosticsUpdated);
        });

        view.condition(&mut cx, |view, cx| view.text(cx).contains("const a"))
            .await;

        view.update(&mut cx, |view, cx| {
            let editor = view.editor.update(cx, |editor, cx| editor.snapshot(cx));

            assert_eq!(
                editor.text(),
                concat!(
                    //
                    // a.rs
                    //
                    "\n", // filename
                    "\n", // padding
                    // diagnostic group 1
                    "\n", // primary message
                    "\n", // padding
                    "const a: i32 = 'a';\n",
                    "\n", // supporting diagnostic
                    "\n", // context line
                    //
                    // main.rs
                    //
                    "\n", // filename
                    "\n", // padding
                    // diagnostic group 1
                    "\n", // primary message
                    "\n", // padding
                    "    let x = vec![];\n",
                    "    let y = vec![];\n",
                    "\n", // supporting diagnostic
                    "    a(x);\n",
                    "    b(y);\n",
                    "\n", // supporting diagnostic
                    "    // comment 1\n",
                    "    // comment 2\n",
                    "    c(y);\n",
                    "\n", // supporting diagnostic
                    "    d(x);\n",
                    // diagnostic group 2
                    "\n", // primary message
                    "\n", // filename
                    "fn main() {\n",
                    "    let x = vec![];\n",
                    "\n", // supporting diagnostic
                    "    let y = vec![];\n",
                    "    a(x);\n",
                    "\n", // supporting diagnostic
                    "    b(y);\n",
                    "\n", // context ellipsis
                    "    c(y);\n",
                    "    d(x);\n",
                    "\n", // supporting diagnostic
                    "}"
                )
            );
        });
    }
}
