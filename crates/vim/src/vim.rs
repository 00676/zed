#[cfg(test)]
mod test_contexts;

mod editor_events;
mod insert;
mod motion;
mod normal;
mod object;
mod state;
mod utils;
mod visual;

use collections::HashMap;
use command_palette::CommandPaletteFilter;
use editor::{Bias, Cancel, CursorShape, Editor};
use gpui::{impl_actions, MutableAppContext, Subscription, ViewContext, WeakViewHandle};
use serde::Deserialize;

use settings::Settings;
use state::{Mode, Operator, VimState};
use workspace::{self, Workspace};

#[derive(Clone, Deserialize, PartialEq)]
pub struct SwitchMode(pub Mode);

#[derive(Clone, Deserialize, PartialEq)]
pub struct PushOperator(pub Operator);

impl_actions!(vim, [SwitchMode, PushOperator]);

pub fn init(cx: &mut MutableAppContext) {
    editor_events::init(cx);
    normal::init(cx);
    visual::init(cx);
    insert::init(cx);
    object::init(cx);
    motion::init(cx);

    // Vim Actions
    cx.add_action(|_: &mut Workspace, &SwitchMode(mode): &SwitchMode, cx| {
        Vim::update(cx, |vim, cx| vim.switch_mode(mode, false, cx))
    });
    cx.add_action(
        |_: &mut Workspace, &PushOperator(operator): &PushOperator, cx| {
            Vim::update(cx, |vim, cx| vim.push_operator(operator, cx))
        },
    );

    // Editor Actions
    cx.add_action(|_: &mut Editor, _: &Cancel, cx| {
        // If we are in a non normal mode or have an active operator, swap to normal mode
        // Otherwise forward cancel on to the editor
        let vim = Vim::read(cx);
        if vim.state.mode != Mode::Normal || vim.active_operator().is_some() {
            MutableAppContext::defer(cx, |cx| {
                Vim::update(cx, |state, cx| {
                    state.switch_mode(Mode::Normal, false, cx);
                });
            });
        } else {
            cx.propagate_action();
        }
    });

    // Sync initial settings with the rest of the app
    Vim::update(cx, |state, cx| state.sync_vim_settings(cx));

    // Any time settings change, update vim mode to match
    cx.observe_global::<Settings, _>(|cx| {
        Vim::update(cx, |state, cx| {
            state.set_enabled(cx.global::<Settings>().vim_mode, cx)
        })
    })
    .detach();
}

#[derive(Default)]
pub struct Vim {
    editors: HashMap<usize, WeakViewHandle<Editor>>,
    active_editor: Option<WeakViewHandle<Editor>>,
    selection_subscription: Option<Subscription>,

    enabled: bool,
    state: VimState,
}

impl Vim {
    fn read(cx: &mut MutableAppContext) -> &Self {
        cx.default_global()
    }

    fn update<F, S>(cx: &mut MutableAppContext, update: F) -> S
    where
        F: FnOnce(&mut Self, &mut MutableAppContext) -> S,
    {
        cx.update_default_global(update)
    }

    fn update_active_editor<S>(
        &self,
        cx: &mut MutableAppContext,
        update: impl FnOnce(&mut Editor, &mut ViewContext<Editor>) -> S,
    ) -> Option<S> {
        self.active_editor
            .clone()
            .and_then(|ae| ae.upgrade(cx))
            .map(|ae| ae.update(cx, update))
    }

    fn switch_mode(&mut self, mode: Mode, leave_selections: bool, cx: &mut MutableAppContext) {
        self.state.mode = mode;
        self.state.operator_stack.clear();

        // Sync editor settings like clip mode
        self.sync_vim_settings(cx);

        if leave_selections {
            return;
        }

        // Adjust selections
        for editor in self.editors.values() {
            if let Some(editor) = editor.upgrade(cx) {
                editor.update(cx, |editor, cx| {
                    editor.change_selections(None, cx, |s| {
                        s.move_with(|map, selection| {
                            if self.state.empty_selections_only() {
                                let new_head = map.clip_point(selection.head(), Bias::Left);
                                selection.collapse_to(new_head, selection.goal)
                            } else {
                                selection.set_head(
                                    map.clip_point(selection.head(), Bias::Left),
                                    selection.goal,
                                );
                            }
                        });
                    })
                })
            }
        }
    }

    fn push_operator(&mut self, operator: Operator, cx: &mut MutableAppContext) {
        self.state.operator_stack.push(operator);
        self.sync_vim_settings(cx);
    }

    fn pop_operator(&mut self, cx: &mut MutableAppContext) -> Operator {
        let popped_operator = self.state.operator_stack.pop()
            .expect("Operator popped when no operator was on the stack. This likely means there is an invalid keymap config");
        self.sync_vim_settings(cx);
        popped_operator
    }

    fn clear_operator(&mut self, cx: &mut MutableAppContext) {
        self.state.operator_stack.clear();
        self.sync_vim_settings(cx);
    }

    fn active_operator(&self) -> Option<Operator> {
        self.state.operator_stack.last().copied()
    }

    fn set_enabled(&mut self, enabled: bool, cx: &mut MutableAppContext) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.state = Default::default();
            if enabled {
                self.switch_mode(Mode::Normal, false, cx);
            }
            self.sync_vim_settings(cx);
        }
    }

    fn sync_vim_settings(&self, cx: &mut MutableAppContext) {
        let state = &self.state;
        let cursor_shape = state.cursor_shape();

        cx.update_default_global::<CommandPaletteFilter, _, _>(|filter, _| {
            if self.enabled {
                filter.filtered_namespaces.remove("vim");
            } else {
                filter.filtered_namespaces.insert("vim");
            }
        });

        for editor in self.editors.values() {
            if let Some(editor) = editor.upgrade(cx) {
                editor.update(cx, |editor, cx| {
                    if self.enabled {
                        editor.set_cursor_shape(cursor_shape, cx);
                        editor.set_clip_at_line_ends(state.clip_at_line_end(), cx);
                        editor.set_input_enabled(!state.vim_controlled());
                        editor.selections.line_mode =
                            matches!(state.mode, Mode::Visual { line: true });
                        let context_layer = state.keymap_context_layer();
                        editor.set_keymap_context_layer::<Self>(context_layer);
                    } else {
                        editor.set_cursor_shape(CursorShape::Bar, cx);
                        editor.set_clip_at_line_ends(false, cx);
                        editor.set_input_enabled(true);
                        editor.selections.line_mode = false;
                        editor.remove_keymap_context_layer::<Self>();
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod test {
    use indoc::indoc;
    use search::BufferSearchBar;

    use crate::{
        state::Mode,
        test_contexts::{NeovimBackedTestContext, VimTestContext},
    };

    #[gpui::test]
    async fn test_initially_disabled(cx: &mut gpui::TestAppContext) {
        let mut cx = VimTestContext::new(cx, false).await;
        cx.simulate_keystrokes(["h", "j", "k", "l"]);
        cx.assert_editor_state("hjklˇ");
    }

    #[gpui::test]
    async fn test_neovim(cx: &mut gpui::TestAppContext) {
        let mut cx = NeovimBackedTestContext::new("test_neovim", cx).await;

        cx.simulate_shared_keystroke("i").await;
        cx.simulate_shared_keystrokes([
            "shift-T", "e", "s", "t", " ", "t", "e", "s", "t", "escape", "0", "d", "w",
        ])
        .await;
        cx.assert_state_matches().await;
        cx.assert_editor_state("ˇtest");
    }

    #[gpui::test]
    async fn test_toggle_through_settings(cx: &mut gpui::TestAppContext) {
        let mut cx = VimTestContext::new(cx, true).await;

        cx.simulate_keystroke("i");
        assert_eq!(cx.mode(), Mode::Insert);

        // Editor acts as though vim is disabled
        cx.disable_vim();
        cx.simulate_keystrokes(["h", "j", "k", "l"]);
        cx.assert_editor_state("hjklˇ");

        // Selections aren't changed if editor is blurred but vim-mode is still disabled.
        cx.set_state("«hjklˇ»", Mode::Normal);
        cx.assert_editor_state("«hjklˇ»");
        cx.update_editor(|_, cx| cx.blur());
        cx.assert_editor_state("«hjklˇ»");
        cx.update_editor(|_, cx| cx.focus_self());
        cx.assert_editor_state("«hjklˇ»");

        // Enabling dynamically sets vim mode again and restores normal mode
        cx.enable_vim();
        assert_eq!(cx.mode(), Mode::Normal);
        cx.simulate_keystrokes(["h", "h", "h", "l"]);
        assert_eq!(cx.buffer_text(), "hjkl".to_owned());
        cx.assert_editor_state("hˇjkl");
        cx.simulate_keystrokes(["i", "T", "e", "s", "t"]);
        cx.assert_editor_state("hTestˇjkl");

        // Disabling and enabling resets to normal mode
        assert_eq!(cx.mode(), Mode::Insert);
        cx.disable_vim();
        cx.enable_vim();
        assert_eq!(cx.mode(), Mode::Normal);
    }

    #[gpui::test]
    async fn test_buffer_search(cx: &mut gpui::TestAppContext) {
        let mut cx = VimTestContext::new(cx, true).await;

        cx.set_state(
            indoc! {"
            The quick brown
            fox juˇmps over
            the lazy dog"},
            Mode::Normal,
        );
        cx.simulate_keystroke("/");

        // We now use a weird insert mode with selection when jumping to a single line editor
        assert_eq!(cx.mode(), Mode::Insert);

        let search_bar = cx.workspace(|workspace, cx| {
            workspace
                .active_pane()
                .read(cx)
                .toolbar()
                .read(cx)
                .item_of_type::<BufferSearchBar>()
                .expect("Buffer search bar should be deployed")
        });

        search_bar.read_with(cx.cx, |bar, cx| {
            assert_eq!(bar.query_editor.read(cx).text(cx), "jumps");
        })
    }
}
