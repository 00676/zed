use std::sync::Arc;

use alacritty_terminal::ansi::Handler;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::{
    config::{Config, Program, PtyConfig},
    event::{Event, EventListener, Notify},
    event_loop::{EventLoop, Notifier},
    grid::Indexed,
    index::Point,
    sync::FairMutex,
    term::{cell::Cell, SizeInfo},
    tty, Term,
};
use futures::{
    channel::mpsc::{unbounded, UnboundedSender},
    StreamExt,
};
use gpui::{
    actions,
    color::Color,
    elements::*,
    fonts::{with_font_cache, TextStyle},
    geometry::{rect::RectF, vector::vec2f},
    impl_internal_actions,
    keymap::Keystroke,
    text_layout::Line,
    Entity,
    Event::KeyDown,
    MutableAppContext, View, ViewContext,
};
use project::{Project, ProjectPath};
use settings::Settings;
use smallvec::SmallVec;
use workspace::{Item, Workspace};

mod event_listener;

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct KeyInput(String);
// #[derive(Clone, Default, Debug, PartialEq, Eq)]
// struct Backspace;
//Action steps:
//Create an action struct with actions!
//Create an action handler that accepts that struct as an arg
//Register that handler in `init`
//If adding to key map file, reference the *struct name*, not the *handler function*
actions!(terminal, [Deploy, Backspace, Execute]); //This is a shortcut for unit structs
impl_internal_actions!(terminal, [KeyInput]); //For actions that don't need to be serialized

pub fn init(cx: &mut MutableAppContext) {
    cx.add_action(TerminalView::deploy);
    cx.add_action(TerminalView::handle_key_input);
    cx.add_action(TerminalView::backspace);
    cx.add_action(TerminalView::execute);
}

#[derive(Clone)]
pub struct ZedListener(UnboundedSender<Event>);

impl EventListener for ZedListener {
    fn send_event(&self, event: Event) {
        self.0.unbounded_send(event).ok();
    }
}

struct TerminalView {
    loop_tx: Notifier,
    term: Arc<FairMutex<Term<ZedListener>>>,
    title: String,
    cur_command: String,
}

impl Entity for TerminalView {
    type Event = ();
}

impl TerminalView {
    fn new(cx: &mut ViewContext<Self>) -> Self {
        let (events_tx, mut events_rx) = unbounded();
        cx.spawn(|this, mut cx| async move {
            //TODO cleanup
            while let Some(event) = events_rx.next().await {
                this.update(&mut cx, |this, cx| {
                    this.process_terminal_event(event, cx);
                    cx.notify();
                });
            }
        })
        .detach();

        let pty_config = PtyConfig {
            shell: Some(Program::Just("zsh".to_string())),
            working_directory: None,
            hold: false,
        };

        let config = Config {
            pty_config: pty_config.clone(),
            ..Default::default()
        };
        let size_info = SizeInfo::new(400., 100.0, 5., 5., 0., 0., false);

        let term = Term::new(&config, size_info, ZedListener(events_tx.clone()));
        let term = Arc::new(FairMutex::new(term));

        let pty = tty::new(&pty_config, &size_info, None).expect("Could not create tty");

        let event_loop = EventLoop::new(
            term.clone(),
            ZedListener(events_tx.clone()),
            pty,
            pty_config.hold,
            false,
        );

        //This variable is how we send stuff to Alacritty
        //Need to wrap it up in a message, which is done by Notifier
        let loop_tx = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn(); //todo cleanup

        // let term = Arc::new(Mutex::new(ZedTerminal::new()));
        // cx.set_global(term.clone());
        TerminalView {
            title: "Terminal".to_string(),
            term,
            loop_tx,
            cur_command: "".to_string(),
        }
    }

    fn deploy(workspace: &mut Workspace, _: &Deploy, cx: &mut ViewContext<Workspace>) {
        workspace.add_item(Box::new(cx.add_view(|cx| TerminalView::new(cx))), cx);
    }

    fn process_terminal_event(
        &mut self,
        event: alacritty_terminal::event::Event,
        cx: &mut ViewContext<Self>,
    ) {
        match event {
            alacritty_terminal::event::Event::Wakeup => cx.notify(),
            alacritty_terminal::event::Event::PtyWrite(out) => {
                self.loop_tx.notify(out.into_bytes())
            }
            _ => {}
        }
        //
    }

    //
    fn handle_key_input(&mut self, action: &KeyInput, cx: &mut ViewContext<Self>) {
        let c = action.0.chars().next().unwrap();
        let mut bytes = vec![0; c.len_utf8()];
        c.encode_utf8(&mut bytes[..]);
        self.loop_tx.notify(bytes);
    }

    fn backspace(&mut self, action: &Backspace, cx: &mut ViewContext<Self>) {
        let c = 127u8 as char;
        let mut bytes = vec![0; c.len_utf8()];
        c.encode_utf8(&mut bytes[..]);
        self.loop_tx.notify(bytes);
    }

    fn execute(&mut self, action: &Execute, cx: &mut ViewContext<Self>) {
        let c = 13u8 as char;
        let mut bytes = vec![0; c.len_utf8()];
        c.encode_utf8(&mut bytes[..]);
        self.loop_tx.notify(bytes);
        self.term.lock().scroll_display(Scroll::Bottom);
    }
}

impl View for TerminalView {
    fn ui_name() -> &'static str {
        "TerminalView"
    }

    fn render(&mut self, cx: &mut gpui::RenderContext<'_, Self>) -> ElementBox {
        let _theme = cx.global::<Settings>().theme.clone();

        TerminalEl::new(self.term.clone())
            .contained()
            // .with_style(theme.terminal.container)
            .boxed()
    }
}

struct TerminalEl {
    term: Arc<FairMutex<Term<ZedListener>>>,
}

impl TerminalEl {
    fn new(term: Arc<FairMutex<Term<ZedListener>>>) -> TerminalEl {
        TerminalEl { term }
    }
}

struct LayoutState {
    lines: Vec<Line>,
    line_height: f32,
}

impl Element for TerminalEl {
    type LayoutState = LayoutState;
    type PaintState = ();

    fn layout(
        &mut self,
        constraint: gpui::SizeConstraint,
        cx: &mut gpui::LayoutContext,
    ) -> (gpui::geometry::vector::Vector2F, Self::LayoutState) {
        let term = self.term.lock();
        let content = term.renderable_content();

        let mut lines = vec![];
        let mut cur_line = vec![];
        let mut last_line = 0;
        for cell in content.display_iter {
            let Indexed {
                point: Point { line, .. },
                cell: Cell { c, .. },
            } = cell;

            if line != last_line {
                lines.push(cur_line);
                cur_line = vec![];
                last_line = line.0;
            }
            cur_line.push(c);
        }
        let line = lines
            .into_iter()
            .map(|char_vec| char_vec.into_iter().collect::<String>())
            .fold("".to_string(), |grid, line| grid + &line + "\n");

        let chunks = vec![(&line[..], None)].into_iter();

        let text_style = with_font_cache(cx.font_cache.clone(), || TextStyle {
            color: Color::white(),
            ..Default::default()
        });

        let shaped_lines = layout_highlighted_chunks(
            chunks,
            &text_style,
            cx.text_layout_cache,
            &cx.font_cache,
            usize::MAX,
            line.matches('\n').count() + 1,
        );
        let line_height = cx.font_cache.line_height(text_style.font_size);

        (
            constraint.max,
            LayoutState {
                lines: shaped_lines,
                line_height,
            },
        )
    }

    fn paint(
        &mut self,
        bounds: gpui::geometry::rect::RectF,
        visible_bounds: gpui::geometry::rect::RectF,
        layout: &mut Self::LayoutState,
        cx: &mut gpui::PaintContext,
    ) -> Self::PaintState {
        let mut origin = bounds.origin();

        for line in &layout.lines {
            let boundaries = RectF::new(origin, vec2f(bounds.width(), layout.line_height));

            if boundaries.intersects(visible_bounds) {
                line.paint(origin, visible_bounds, layout.line_height, cx);
            }

            origin.set_y(boundaries.max_y());
        }
    }

    fn dispatch_event(
        &mut self,
        event: &gpui::Event,
        _bounds: gpui::geometry::rect::RectF,
        _visible_bounds: gpui::geometry::rect::RectF,
        _layout: &mut Self::LayoutState,
        _paint: &mut Self::PaintState,
        cx: &mut gpui::EventContext,
    ) -> bool {
        match event {
            KeyDown {
                input: Some(input), ..
            } => {
                cx.dispatch_action(KeyInput(input.clone()));
                true
            }
            KeyDown {
                keystroke: Keystroke { key, .. },
                input: None,
                ..
            } => {
                if key == "backspace" {
                    cx.dispatch_action(Backspace);
                    true
                } else if key == "enter" {
                    cx.dispatch_action(Execute);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn debug(
        &self,
        _bounds: gpui::geometry::rect::RectF,
        _layout: &Self::LayoutState,
        _paint: &Self::PaintState,
        _cx: &gpui::DebugContext,
    ) -> gpui::serde_json::Value {
        unreachable!("Should never be called hopefully")
    }
}

///Item is what workspace uses for deciding what to render in a pane
///Often has a file path or somesuch
impl Item for TerminalView {
    fn tab_content(&self, style: &theme::Tab, cx: &gpui::AppContext) -> ElementBox {
        let settings = cx.global::<Settings>();
        let search_theme = &settings.theme.search;
        Flex::row()
            .with_child(
                Label::new(self.title.clone(), style.label.clone())
                    .aligned()
                    .contained()
                    .with_margin_left(search_theme.tab_icon_spacing)
                    .boxed(),
            )
            .boxed()
    }

    fn project_path(&self, _cx: &gpui::AppContext) -> Option<ProjectPath> {
        None
    }

    fn project_entry_ids(&self, _cx: &gpui::AppContext) -> SmallVec<[project::ProjectEntryId; 3]> {
        todo!()
    }

    fn is_singleton(&self, _cx: &gpui::AppContext) -> bool {
        false
    }

    fn set_nav_history(&mut self, _: workspace::ItemNavHistory, _: &mut ViewContext<Self>) {}

    fn can_save(&self, _cx: &gpui::AppContext) -> bool {
        false
    }

    fn save(
        &mut self,
        _project: gpui::ModelHandle<Project>,
        _cx: &mut ViewContext<Self>,
    ) -> gpui::Task<gpui::anyhow::Result<()>> {
        unreachable!("save should not have been called");
    }

    fn save_as(
        &mut self,
        _project: gpui::ModelHandle<Project>,
        _abs_path: std::path::PathBuf,
        _cx: &mut ViewContext<Self>,
    ) -> gpui::Task<gpui::anyhow::Result<()>> {
        unreachable!("save_as should not have been called");
    }

    fn reload(
        &mut self,
        _project: gpui::ModelHandle<Project>,
        _cx: &mut ViewContext<Self>,
    ) -> gpui::Task<gpui::anyhow::Result<()>> {
        gpui::Task::ready(Ok(()))
    }
}
