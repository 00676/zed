use alacritty_terminal::{
    ansi::Color as AnsiColor,
    grid::{GridIterator, Indexed},
    index::Point,
    term::{
        cell::{Cell, Flags},
        SizeInfo,
    },
};
use gpui::{
    color::Color,
    elements::*,
    fonts::{HighlightStyle, TextStyle, Underline},
    geometry::{rect::RectF, vector::vec2f},
    json::json,
    text_layout::Line,
    Event, MouseRegion, PaintContext, Quad, WeakViewHandle,
};
use ordered_float::OrderedFloat;
use settings::Settings;
use std::rc::Rc;
use theme::TerminalStyle;

use crate::{Input, ScrollTerminal, Terminal};

const ALACRITTY_SCROLL_MULTIPLIER: f32 = 3.;

#[cfg(debug_assertions)]
const DEBUG_GRID: bool = false;

pub struct TerminalEl {
    view: WeakViewHandle<Terminal>,
}

impl TerminalEl {
    pub fn new(view: WeakViewHandle<Terminal>) -> TerminalEl {
        TerminalEl { view }
    }
}

pub struct LayoutState {
    lines: Vec<Line>,
    line_height: f32,
    em_width: f32,
    cursor: Option<RectF>,
    cur_size: SizeInfo,
}

impl Element for TerminalEl {
    type LayoutState = LayoutState;
    type PaintState = ();

    fn layout(
        &mut self,
        constraint: gpui::SizeConstraint,
        cx: &mut gpui::LayoutContext,
    ) -> (gpui::geometry::vector::Vector2F, Self::LayoutState) {
        let view = self.view.upgrade(cx).unwrap();
        let size = constraint.max;
        let settings = cx.global::<Settings>();
        let editor_theme = &settings.theme.editor;
        let font_cache = cx.font_cache();

        //Set up text rendering
        let text_style = TextStyle {
            color: editor_theme.text_color,
            font_family_id: settings.buffer_font_family,
            font_family_name: font_cache.family_name(settings.buffer_font_family).unwrap(),
            font_id: font_cache
                .select_font(settings.buffer_font_family, &Default::default())
                .unwrap(),
            font_size: settings.buffer_font_size,
            font_properties: Default::default(),
            underline: Default::default(),
        };

        let line_height = font_cache.line_height(text_style.font_size);
        let cell_width = font_cache.em_advance(text_style.font_id, text_style.font_size);

        let new_size = SizeInfo::new(
            size.x() - cell_width,
            size.y(),
            cell_width,
            line_height,
            0.,
            0.,
            false,
        );
        view.update(cx.app, |view, _cx| {
            view.set_size(new_size);
        });

        let settings = cx.global::<Settings>();
        let terminal_theme = &settings.theme.terminal;
        let term = view.read(cx).term.lock();

        let content = term.renderable_content();
        let (chunks, line_count) = build_chunks(content.display_iter, &terminal_theme);

        let shaped_lines = layout_highlighted_chunks(
            chunks.iter().map(|(text, style)| (text.as_str(), *style)),
            &text_style,
            cx.text_layout_cache,
            &cx.font_cache,
            usize::MAX,
            line_count,
        );

        let cursor_line = content.cursor.point.line.0 + content.display_offset as i32;
        let mut cursor = None;
        if let Some(layout_line) = cursor_line
            .try_into()
            .ok()
            .and_then(|cursor_line: usize| shaped_lines.get(cursor_line))
        {
            let cursor_x = layout_line.x_for_index(content.cursor.point.column.0);
            cursor = Some(RectF::new(
                vec2f(cursor_x, cursor_line as f32 * line_height),
                vec2f(cell_width, line_height),
            ));
        }

        (
            constraint.max,
            LayoutState {
                lines: shaped_lines,
                line_height,
                em_width: cell_width,
                cursor,
                cur_size: new_size,
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
        cx.scene.push_layer(Some(visible_bounds));

        cx.scene.push_mouse_region(MouseRegion {
            view_id: self.view.id(),
            discriminant: None,
            bounds: visible_bounds,
            hover: None,
            mouse_down: Some(Rc::new(|_, cx| cx.focus_parent_view())),
            click: None,
            right_mouse_down: None,
            right_click: None,
            drag: None,
            mouse_down_out: None,
            right_mouse_down_out: None,
        });

        let origin = bounds.origin() + vec2f(layout.em_width, 0.); //Padding

        let mut line_origin = origin;
        for line in &layout.lines {
            let boundaries = RectF::new(line_origin, vec2f(bounds.width(), layout.line_height));

            if boundaries.intersects(visible_bounds) {
                line.paint(line_origin, visible_bounds, layout.line_height, cx);
            }

            line_origin.set_y(boundaries.max_y());
        }

        if let Some(c) = layout.cursor {
            let new_origin = origin + c.origin();
            let new_cursor = RectF::new(new_origin, c.size());
            cx.scene.push_quad(Quad {
                bounds: new_cursor,
                background: Some(Color::white()),
                border: Default::default(),
                corner_radius: 0.,
            });
        }

        #[cfg(debug_assertions)]
        if DEBUG_GRID {
            draw_debug_grid(bounds, layout, cx);
        }

        cx.scene.pop_layer();
    }

    fn dispatch_event(
        &mut self,
        event: &gpui::Event,
        _bounds: gpui::geometry::rect::RectF,
        visible_bounds: gpui::geometry::rect::RectF,
        layout: &mut Self::LayoutState,
        _paint: &mut Self::PaintState,
        cx: &mut gpui::EventContext,
    ) -> bool {
        match event {
            Event::ScrollWheel {
                delta, position, ..
            } => {
                if visible_bounds.contains_point(*position) {
                    let vertical_scroll =
                        (delta.y() / layout.line_height) * ALACRITTY_SCROLL_MULTIPLIER;
                    cx.dispatch_action(ScrollTerminal(vertical_scroll.round() as i32));
                    true
                } else {
                    false
                }
            }
            Event::KeyDown {
                input: Some(input), ..
            } => {
                if cx.is_parent_view_focused() {
                    cx.dispatch_action(Input(input.to_string()));
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
        json!({
            "type": "TerminalElement",
        })
    }
}

pub(crate) fn build_chunks(
    grid_iterator: GridIterator<Cell>,
    theme: &TerminalStyle,
) -> (Vec<(String, Option<HighlightStyle>)>, usize) {
    let mut lines: Vec<(String, Option<HighlightStyle>)> = vec![];
    let mut last_line = 0;
    let mut line_count = 1;
    let mut cur_chunk = String::new();

    let mut cur_highlight = HighlightStyle {
        color: Some(Color::white()),
        ..Default::default()
    };

    for cell in grid_iterator {
        let Indexed {
          point: Point { line, .. },
          cell: Cell {
              c, fg, flags, .. // TODO: Add bg and flags
          }, //TODO: Learn what 'CellExtra does'
      } = cell;

        let new_highlight = make_style_from_cell(fg, flags, theme);

        if line != last_line {
            line_count += 1;
            cur_chunk.push('\n');
            last_line = line.0;
        }

        if new_highlight != cur_highlight {
            lines.push((cur_chunk.clone(), Some(cur_highlight.clone())));
            cur_chunk.clear();
            cur_highlight = new_highlight;
        }
        cur_chunk.push(*c)
    }
    lines.push((cur_chunk, Some(cur_highlight)));
    (lines, line_count)
}

fn make_style_from_cell(fg: &AnsiColor, flags: &Flags, style: &TerminalStyle) -> HighlightStyle {
    let fg = Some(alac_color_to_gpui_color(fg, style));
    let underline = if flags.contains(Flags::UNDERLINE) {
        Some(Underline {
            color: fg,
            squiggly: false,
            thickness: OrderedFloat(1.),
        })
    } else {
        None
    };
    HighlightStyle {
        color: fg,
        underline,
        ..Default::default()
    }
}

fn alac_color_to_gpui_color(allac_color: &AnsiColor, style: &TerminalStyle) -> Color {
    match allac_color {
        alacritty_terminal::ansi::Color::Named(n) => match n {
            alacritty_terminal::ansi::NamedColor::Black => style.black,
            alacritty_terminal::ansi::NamedColor::Red => style.red,
            alacritty_terminal::ansi::NamedColor::Green => style.green,
            alacritty_terminal::ansi::NamedColor::Yellow => style.yellow,
            alacritty_terminal::ansi::NamedColor::Blue => style.blue,
            alacritty_terminal::ansi::NamedColor::Magenta => style.magenta,
            alacritty_terminal::ansi::NamedColor::Cyan => style.cyan,
            alacritty_terminal::ansi::NamedColor::White => style.white,
            alacritty_terminal::ansi::NamedColor::BrightBlack => style.bright_black,
            alacritty_terminal::ansi::NamedColor::BrightRed => style.bright_red,
            alacritty_terminal::ansi::NamedColor::BrightGreen => style.bright_green,
            alacritty_terminal::ansi::NamedColor::BrightYellow => style.bright_yellow,
            alacritty_terminal::ansi::NamedColor::BrightBlue => style.bright_blue,
            alacritty_terminal::ansi::NamedColor::BrightMagenta => style.bright_magenta,
            alacritty_terminal::ansi::NamedColor::BrightCyan => style.bright_cyan,
            alacritty_terminal::ansi::NamedColor::BrightWhite => style.bright_white,
            alacritty_terminal::ansi::NamedColor::Foreground => style.foreground,
            alacritty_terminal::ansi::NamedColor::Background => style.background,
            alacritty_terminal::ansi::NamedColor::Cursor => style.cursor,
            alacritty_terminal::ansi::NamedColor::DimBlack => style.dim_black,
            alacritty_terminal::ansi::NamedColor::DimRed => style.dim_red,
            alacritty_terminal::ansi::NamedColor::DimGreen => style.dim_green,
            alacritty_terminal::ansi::NamedColor::DimYellow => style.dim_yellow,
            alacritty_terminal::ansi::NamedColor::DimBlue => style.dim_blue,
            alacritty_terminal::ansi::NamedColor::DimMagenta => style.dim_magenta,
            alacritty_terminal::ansi::NamedColor::DimCyan => style.dim_cyan,
            alacritty_terminal::ansi::NamedColor::DimWhite => style.dim_white,
            alacritty_terminal::ansi::NamedColor::BrightForeground => style.bright_foreground,
            alacritty_terminal::ansi::NamedColor::DimForeground => style.dim_foreground,
        }, //Theme defined
        alacritty_terminal::ansi::Color::Spec(rgb) => Color::new(rgb.r, rgb.g, rgb.b, 1),
        alacritty_terminal::ansi::Color::Indexed(_) => Color::white(), //Color cube weirdness
    }
}

#[cfg(debug_assertions)]
fn draw_debug_grid(bounds: RectF, layout: &mut LayoutState, cx: &mut PaintContext) {
    let width = layout.cur_size.width();
    let height = layout.cur_size.height();
    //Alacritty uses 'as usize', so shall we.
    for col in 0..(width / layout.em_width).round() as usize {
        cx.scene.push_quad(Quad {
            bounds: RectF::new(
                bounds.origin() + vec2f((col + 1) as f32 * layout.em_width, 0.),
                vec2f(1., height),
            ),
            background: Some(Color::green()),
            border: Default::default(),
            corner_radius: 0.,
        });
    }
    for row in 0..((height / layout.line_height) + 1.0).round() as usize {
        cx.scene.push_quad(Quad {
            bounds: RectF::new(
                bounds.origin() + vec2f(layout.em_width, row as f32 * layout.line_height),
                vec2f(width, 1.),
            ),
            background: Some(Color::green()),
            border: Default::default(),
            corner_radius: 0.,
        });
    }
}
