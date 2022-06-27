use std::ops::Range;

use gpui::{impl_internal_actions, MutableAppContext, Task, ViewContext};
use language::{Bias, ToOffset};
use project::LocationLink;
use settings::Settings;
use util::TryFutureExt;
use workspace::Workspace;

use crate::{Anchor, DisplayPoint, Editor, EditorSnapshot, GoToDefinition, Select, SelectPhase};

#[derive(Clone, PartialEq)]
pub struct UpdateGoToDefinitionLink {
    pub point: Option<DisplayPoint>,
    pub cmd_held: bool,
}

#[derive(Clone, PartialEq)]
pub struct CmdChanged {
    pub cmd_down: bool,
}

#[derive(Clone, PartialEq)]
pub struct GoToFetchedDefinition {
    pub point: DisplayPoint,
}

impl_internal_actions!(
    editor,
    [UpdateGoToDefinitionLink, CmdChanged, GoToFetchedDefinition]
);

pub fn init(cx: &mut MutableAppContext) {
    cx.add_action(update_go_to_definition_link);
    cx.add_action(cmd_changed);
    cx.add_action(go_to_fetched_definition);
}

#[derive(Default)]
pub struct LinkGoToDefinitionState {
    pub last_mouse_location: Option<Anchor>,
    pub symbol_range: Option<Range<Anchor>>,
    pub definitions: Vec<LocationLink>,
    pub task: Option<Task<Option<()>>>,
}

pub fn update_go_to_definition_link(
    editor: &mut Editor,
    &UpdateGoToDefinitionLink { point, cmd_held }: &UpdateGoToDefinitionLink,
    cx: &mut ViewContext<Editor>,
) {
    // Store new mouse point as an anchor
    let snapshot = editor.snapshot(cx);
    let point = point.map(|point| {
        snapshot
            .buffer_snapshot
            .anchor_before(point.to_offset(&snapshot.display_snapshot, Bias::Left))
    });

    // If the new point is the same as the previously stored one, return early
    if let (Some(a), Some(b)) = (
        &point,
        &editor.link_go_to_definition_state.last_mouse_location,
    ) {
        if a.cmp(&b, &snapshot.buffer_snapshot).is_eq() {
            return;
        }
    }

    editor.link_go_to_definition_state.last_mouse_location = point.clone();
    if cmd_held {
        if let Some(point) = point {
            show_link_definition(editor, point, snapshot, cx);
            return;
        }
    }

    hide_link_definition(editor, cx);
}

pub fn cmd_changed(
    editor: &mut Editor,
    &CmdChanged { cmd_down }: &CmdChanged,
    cx: &mut ViewContext<Editor>,
) {
    if let Some(point) = editor
        .link_go_to_definition_state
        .last_mouse_location
        .clone()
    {
        if cmd_down {
            let snapshot = editor.snapshot(cx);
            show_link_definition(editor, point.clone(), snapshot, cx);
        } else {
            hide_link_definition(editor, cx)
        }
    }
}

pub fn show_link_definition(
    editor: &mut Editor,
    trigger_point: Anchor,
    snapshot: EditorSnapshot,
    cx: &mut ViewContext<Editor>,
) {
    if editor.pending_rename.is_some() {
        return;
    }

    let (buffer, buffer_position) = if let Some(output) = editor
        .buffer
        .read(cx)
        .text_anchor_for_position(trigger_point.clone(), cx)
    {
        output
    } else {
        return;
    };

    let excerpt_id = if let Some((excerpt_id, _, _)) = editor
        .buffer()
        .read(cx)
        .excerpt_containing(trigger_point.clone(), cx)
    {
        excerpt_id
    } else {
        return;
    };

    let project = if let Some(project) = editor.project.clone() {
        project
    } else {
        return;
    };

    // Don't request again if the location is within the symbol region of a previous request
    if let Some(symbol_range) = &editor.link_go_to_definition_state.symbol_range {
        if symbol_range
            .start
            .cmp(&trigger_point, &snapshot.buffer_snapshot)
            .is_le()
            && symbol_range
                .end
                .cmp(&trigger_point, &snapshot.buffer_snapshot)
                .is_ge()
        {
            return;
        }
    }

    let task = cx.spawn_weak(|this, mut cx| {
        async move {
            // query the LSP for definition info
            let definition_request = cx.update(|cx| {
                project.update(cx, |project, cx| {
                    project.definition(&buffer, buffer_position.clone(), cx)
                })
            });

            let result = definition_request.await.ok().map(|definition_result| {
                (
                    definition_result.iter().find_map(|link| {
                        link.origin.as_ref().map(|origin| {
                            let start = snapshot
                                .buffer_snapshot
                                .anchor_in_excerpt(excerpt_id.clone(), origin.range.start);
                            let end = snapshot
                                .buffer_snapshot
                                .anchor_in_excerpt(excerpt_id.clone(), origin.range.end);

                            start..end
                        })
                    }),
                    definition_result,
                )
            });

            if let Some(this) = this.upgrade(&cx) {
                this.update(&mut cx, |this, cx| {
                    // Clear any existing highlights
                    this.clear_text_highlights::<LinkGoToDefinitionState>(cx);
                    this.link_go_to_definition_state.symbol_range = result
                        .as_ref()
                        .and_then(|(symbol_range, _)| symbol_range.clone());

                    if let Some((symbol_range, definitions)) = result {
                        this.link_go_to_definition_state.definitions = definitions.clone();

                        let buffer_snapshot = buffer.read(cx).snapshot();
                        // Only show highlight if there exists a definition to jump to that doesn't contain
                        // the current location.
                        if definitions.iter().any(|definition| {
                            let target = &definition.target;
                            if target.buffer == buffer {
                                let range = &target.range;
                                // Expand range by one character as lsp definition ranges include positions adjacent
                                // but not contained by the symbol range
                                let start = buffer_snapshot.clip_offset(
                                    range.start.to_offset(&buffer_snapshot).saturating_sub(1),
                                    Bias::Left,
                                );
                                let end = buffer_snapshot.clip_offset(
                                    range.end.to_offset(&buffer_snapshot) + 1,
                                    Bias::Right,
                                );
                                let offset = buffer_position.to_offset(&buffer_snapshot);
                                !(start <= offset && end >= offset)
                            } else {
                                true
                            }
                        }) {
                            // If no symbol range returned from language server, use the surrounding word.
                            let highlight_range = symbol_range.unwrap_or_else(|| {
                                let snapshot = &snapshot.buffer_snapshot;
                                let (offset_range, _) = snapshot.surrounding_word(trigger_point);

                                snapshot.anchor_before(offset_range.start)
                                    ..snapshot.anchor_after(offset_range.end)
                            });

                            // Highlight symbol using theme link definition highlight style
                            let style = cx.global::<Settings>().theme.editor.link_definition;
                            this.highlight_text::<LinkGoToDefinitionState>(
                                vec![highlight_range],
                                style,
                                cx,
                            )
                        } else {
                            hide_link_definition(this, cx);
                        }
                    }
                })
            }

            Ok::<_, anyhow::Error>(())
        }
        .log_err()
    });

    editor.link_go_to_definition_state.task = Some(task);
}

pub fn hide_link_definition(editor: &mut Editor, cx: &mut ViewContext<Editor>) {
    if editor.link_go_to_definition_state.symbol_range.is_some()
        || !editor.link_go_to_definition_state.definitions.is_empty()
    {
        editor.link_go_to_definition_state.symbol_range.take();
        editor.link_go_to_definition_state.definitions.clear();
        cx.notify();
    }

    editor.link_go_to_definition_state.task = None;

    editor.clear_text_highlights::<LinkGoToDefinitionState>(cx);
}

pub fn go_to_fetched_definition(
    workspace: &mut Workspace,
    GoToFetchedDefinition { point }: &GoToFetchedDefinition,
    cx: &mut ViewContext<Workspace>,
) {
    let active_item = workspace.active_item(cx);
    let editor_handle = if let Some(editor) = active_item
        .as_ref()
        .and_then(|item| item.act_as::<Editor>(cx))
    {
        editor
    } else {
        return;
    };

    let definitions = editor_handle.update(cx, |editor, cx| {
        let definitions = editor.link_go_to_definition_state.definitions.clone();
        hide_link_definition(editor, cx);
        definitions
    });

    if !definitions.is_empty() {
        Editor::navigate_to_definitions(workspace, editor_handle, definitions, cx);
    } else {
        editor_handle.update(cx, |editor, cx| {
            editor.select(
                &Select(SelectPhase::Begin {
                    position: point.clone(),
                    add: false,
                    click_count: 1,
                }),
                cx,
            );
        });

        Editor::go_to_definition(workspace, &GoToDefinition, cx);
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use indoc::indoc;

    use crate::test::EditorLspTestContext;

    use super::*;

    #[gpui::test]
    async fn test_link_go_to_definition(cx: &mut gpui::TestAppContext) {
        let mut cx = EditorLspTestContext::new_rust(
            lsp::ServerCapabilities {
                hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            cx,
        )
        .await;

        cx.set_state(indoc! {"
            fn |test()
                do_work();
            
            fn do_work()
                test();"});

        // Basic hold cmd, expect highlight in region if response contains definition
        let hover_point = cx.display_point(indoc! {"
            fn test()
                do_w|ork();
            
            fn do_work()
                test();"});

        let symbol_range = cx.lsp_range(indoc! {"
            fn test()
                [do_work]();
            
            fn do_work()
                test();"});
        let target_range = cx.lsp_range(indoc! {"
            fn test()
                do_work();
            
            fn [do_work]()
                test();"});

        let mut requests =
            cx.lsp
                .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                    Ok(Some(lsp::GotoDefinitionResponse::Link(vec![
                        lsp::LocationLink {
                            origin_selection_range: Some(symbol_range),
                            target_uri: lsp::Url::from_file_path("/root/dir/file.rs").unwrap(),
                            target_range,
                            target_selection_range: target_range,
                        },
                    ])))
                });
        cx.update_editor(|editor, cx| {
            update_go_to_definition_link(
                editor,
                &UpdateGoToDefinitionLink {
                    point: Some(hover_point),
                    cmd_held: true,
                },
                cx,
            );
        });
        requests.next().await;
        cx.foreground().run_until_parked();
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                [do_work]();
            
            fn do_work()
                test();"});

        // Unpress cmd causes highlight to go away
        cx.update_editor(|editor, cx| {
            cmd_changed(editor, &CmdChanged { cmd_down: false }, cx);
        });
        // Assert no link highlights
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                test();"});

        // Response without source range still highlights word
        cx.update_editor(|editor, _| editor.link_go_to_definition_state.last_mouse_location = None);
        let mut requests =
            cx.lsp
                .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                    Ok(Some(lsp::GotoDefinitionResponse::Link(vec![
                        lsp::LocationLink {
                            // No origin range
                            origin_selection_range: None,
                            target_uri: lsp::Url::from_file_path("/root/dir/file.rs").unwrap(),
                            target_range,
                            target_selection_range: target_range,
                        },
                    ])))
                });
        cx.update_editor(|editor, cx| {
            update_go_to_definition_link(
                editor,
                &UpdateGoToDefinitionLink {
                    point: Some(hover_point),
                    cmd_held: true,
                },
                cx,
            );
        });
        requests.next().await;
        cx.foreground().run_until_parked();

        println!("tag");
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                [do_work]();
            
            fn do_work()
                test();"});

        // Moving mouse to location with no response dismisses highlight
        let hover_point = cx.display_point(indoc! {"
            f|n test()
                do_work();
            
            fn do_work()
                test();"});
        let mut requests =
            cx.lsp
                .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                    // No definitions returned
                    Ok(Some(lsp::GotoDefinitionResponse::Link(vec![])))
                });
        cx.update_editor(|editor, cx| {
            update_go_to_definition_link(
                editor,
                &UpdateGoToDefinitionLink {
                    point: Some(hover_point),
                    cmd_held: true,
                },
                cx,
            );
        });
        requests.next().await;
        cx.foreground().run_until_parked();

        // Assert no link highlights
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                test();"});

        // Move mouse without cmd and then pressing cmd triggers highlight
        let hover_point = cx.display_point(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                te|st();"});
        cx.update_editor(|editor, cx| {
            update_go_to_definition_link(
                editor,
                &UpdateGoToDefinitionLink {
                    point: Some(hover_point),
                    cmd_held: false,
                },
                cx,
            );
        });
        cx.foreground().run_until_parked();

        // Assert no link highlights
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                test();"});

        let symbol_range = cx.lsp_range(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                [test]();"});
        let target_range = cx.lsp_range(indoc! {"
            fn [test]()
                do_work();
            
            fn do_work()
                test();"});

        let mut requests =
            cx.lsp
                .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                    Ok(Some(lsp::GotoDefinitionResponse::Link(vec![
                        lsp::LocationLink {
                            origin_selection_range: Some(symbol_range),
                            target_uri: lsp::Url::from_file_path("/root/dir/file.rs").unwrap(),
                            target_range,
                            target_selection_range: target_range,
                        },
                    ])))
                });
        cx.update_editor(|editor, cx| {
            cmd_changed(editor, &CmdChanged { cmd_down: true }, cx);
        });
        requests.next().await;
        cx.foreground().run_until_parked();

        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                [test]();"});

        // Moving within symbol range doesn't re-request
        let hover_point = cx.display_point(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                tes|t();"});
        cx.update_editor(|editor, cx| {
            update_go_to_definition_link(
                editor,
                &UpdateGoToDefinitionLink {
                    point: Some(hover_point),
                    cmd_held: true,
                },
                cx,
            );
        });
        cx.foreground().run_until_parked();
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                [test]();"});

        // Cmd click with existing definition doesn't re-request and dismisses highlight
        cx.update_workspace(|workspace, cx| {
            go_to_fetched_definition(workspace, &GoToFetchedDefinition { point: hover_point }, cx);
        });
        // Assert selection moved to to definition
        cx.lsp
            .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                // Empty definition response to make sure we aren't hitting the lsp and using
                // the cached location instead
                Ok(Some(lsp::GotoDefinitionResponse::Link(vec![])))
            });
        cx.assert_editor_state(indoc! {"
            fn [test}()
                do_work();
            
            fn do_work()
                test();"});
        // Assert no link highlights after jump
        cx.assert_editor_text_highlights::<LinkGoToDefinitionState>(indoc! {"
            fn test()
                do_work();
            
            fn do_work()
                test();"});

        // Cmd click without existing definition requests and jumps
        let hover_point = cx.display_point(indoc! {"
            fn test()
                do_w|ork();
            
            fn do_work()
                test();"});
        let target_range = cx.lsp_range(indoc! {"
            fn test()
                do_work();
            
            fn [do_work]()
                test();"});

        let mut requests =
            cx.lsp
                .handle_request::<lsp::request::GotoDefinition, _, _>(move |_, _| async move {
                    Ok(Some(lsp::GotoDefinitionResponse::Link(vec![
                        lsp::LocationLink {
                            origin_selection_range: None,
                            target_uri: lsp::Url::from_file_path("/root/dir/file.rs").unwrap(),
                            target_range,
                            target_selection_range: target_range,
                        },
                    ])))
                });
        cx.update_workspace(|workspace, cx| {
            go_to_fetched_definition(workspace, &GoToFetchedDefinition { point: hover_point }, cx);
        });
        requests.next().await;
        cx.foreground().run_until_parked();

        cx.assert_editor_state(indoc! {"
            fn test()
                do_work();
            
            fn [do_work}()
                test();"});
    }
}
