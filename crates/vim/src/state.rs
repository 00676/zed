use editor::CursorShape;
use gpui::keymap::Context;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Mode {
    Normal,
    Insert,
    Visual { line: bool },
}

impl Default for Mode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Namespace {
    G,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Operator {
    Namespace(Namespace),
    Change,
    Delete,
    Yank,
}

#[derive(Default)]
pub struct VimState {
    pub mode: Mode,
    pub operator_stack: Vec<Operator>,
}

impl VimState {
    pub fn cursor_shape(&self) -> CursorShape {
        match self.mode {
            Mode::Normal => {
                if self.operator_stack.is_empty() {
                    CursorShape::Block
                } else {
                    CursorShape::Underscore
                }
            }
            Mode::Visual { .. } => CursorShape::Block,
            Mode::Insert => CursorShape::Bar,
        }
    }

    pub fn vim_controlled(&self) -> bool {
        !matches!(self.mode, Mode::Insert)
    }

    pub fn clip_at_line_end(&self) -> bool {
        match self.mode {
            Mode::Insert | Mode::Visual { .. } => false,
            _ => true,
        }
    }

    pub fn empty_selections_only(&self) -> bool {
        !matches!(self.mode, Mode::Visual { .. })
    }

    pub fn keymap_context_layer(&self) -> Context {
        let mut context = Context::default();
        context.map.insert(
            "vim_mode".to_string(),
            match self.mode {
                Mode::Normal => "normal",
                Mode::Visual { .. } => "visual",
                Mode::Insert => "insert",
            }
            .to_string(),
        );

        if self.vim_controlled() {
            context.set.insert("VimControl".to_string());
        }

        Operator::set_context(self.operator_stack.last(), &mut context);

        context
    }
}

impl Operator {
    pub fn set_context(operator: Option<&Operator>, context: &mut Context) {
        let operator_context = match operator {
            Some(Operator::Namespace(Namespace::G)) => "g",
            Some(Operator::Change) => "c",
            Some(Operator::Delete) => "d",
            Some(Operator::Yank) => "y",
            None => "none",
        }
        .to_owned();

        context
            .map
            .insert("vim_operator".to_string(), operator_context.to_string());
    }
}
