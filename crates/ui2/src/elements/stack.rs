use gpui2::{div, Div};

use crate::prelude::*;

pub trait Stack: Styled + Sized {
    /// Horizontally stacks elements.
    fn h_stack(self) -> Self {
        self.flex().flex_row().items_center()
    }

    /// Vertically stacks elements.
    fn v_stack(self) -> Self {
        self.flex().flex_col()
    }
}

impl<S: 'static + Send + Sync> Stack for Div<S> {}

/// Horizontally stacks elements.
///
/// Sets `flex()`, `flex_row()`, `items_center()`
pub fn h_stack<S: 'static + Send + Sync>() -> Div<S> {
    div().h_stack()
}

/// Vertically stacks elements.
///
/// Sets `flex()`, `flex_col()`
pub fn v_stack<S: 'static + Send + Sync>() -> Div<S> {
    div().v_stack()
}
