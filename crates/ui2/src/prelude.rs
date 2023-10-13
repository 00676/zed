pub use gpui3::{
    div, Element, IntoAnyElement, ParentElement, ScrollState, StyleHelpers, Styled, ViewContext,
    WindowContext,
};

pub use crate::{theme, ButtonVariant, ElementExt, Theme};

use gpui3::{hsla, rems, rgb, AbsoluteLength, Hsla};
use strum::EnumIter;

#[derive(Clone, Copy)]
pub struct Token {
    pub list_indent_depth: AbsoluteLength,
    pub default_panel_size: AbsoluteLength,
    pub state_hover_background: Hsla,
    pub state_active_background: Hsla,
}

impl Default for Token {
    fn default() -> Self {
        Self {
            list_indent_depth: AbsoluteLength::Rems(rems(0.3)),
            default_panel_size: AbsoluteLength::Rems(rems(16.)),
            state_hover_background: hsla(0.0, 0.0, 0.0, 0.08),
            state_active_background: hsla(0.0, 0.0, 0.0, 0.16),
        }
    }
}

pub fn token() -> Token {
    Token::default()
}

#[derive(Default)]
pub struct SystemColor {
    pub transparent: Hsla,
    pub mac_os_traffic_light_red: Hsla,
    pub mac_os_traffic_light_yellow: Hsla,
    pub mac_os_traffic_light_green: Hsla,
}

impl SystemColor {
    pub fn new() -> SystemColor {
        SystemColor {
            transparent: hsla(0.0, 0.0, 0.0, 0.0),
            mac_os_traffic_light_red: rgb::<Hsla>(0xEC695E),
            mac_os_traffic_light_yellow: rgb::<Hsla>(0xF4BF4F),
            mac_os_traffic_light_green: rgb::<Hsla>(0x62C554),
        }
    }
    pub fn color(&self) -> Hsla {
        self.transparent
    }
}

#[derive(Clone, Copy)]
pub struct ThemeColor {
    pub border: Hsla,
    pub border_variant: Hsla,
    pub border_focused: Hsla,
    pub border_transparent: Hsla,
    /// The background color of an elevated surface, like a modal, tooltip or toast.
    pub elevated_surface: Hsla,
    pub surface: Hsla,
    /// Default background for elements like filled buttons,
    /// text fields, checkboxes, radio buttons, etc.
    /// - TODO: Map to step 3.
    pub filled_element: Hsla,
    /// The background color of a hovered element, like a button being hovered
    /// with a mouse, or hovered on a touch screen.
    /// - TODO: Map to step 4.
    pub filled_element_hover: Hsla,
    /// The background color of an active element, like a button being pressed,
    /// or tapped on a touch screen.
    /// - TODO: Map to step 5.
    pub filled_element_active: Hsla,
    /// The background color of a selected element, like a selected tab, a button toggled on, or a checkbox that is checked.
    pub filled_element_selected: Hsla,
    pub filled_element_disabled: Hsla,
    pub ghost_element: Hsla,
    /// - TODO: Map to step 3.
    pub ghost_element_hover: Hsla,
    /// - TODO: Map to step 4.
    pub ghost_element_active: Hsla,
    pub ghost_element_selected: Hsla,
    pub ghost_element_disabled: Hsla,
}

impl ThemeColor {
    pub fn new(cx: &WindowContext) -> Self {
        let theme = theme(cx);
        let system_color = SystemColor::new();

        Self {
            border: theme.lowest.base.default.border,
            border_variant: theme.lowest.variant.default.border,
            border_focused: theme.lowest.accent.default.border,
            border_transparent: system_color.transparent,
            elevated_surface: theme.middle.base.default.background,
            surface: theme.middle.base.default.background,
            filled_element: theme.lowest.base.default.background,
            filled_element_hover: theme.lowest.base.hovered.background,
            filled_element_active: theme.lowest.base.active.background,
            filled_element_selected: theme.lowest.accent.default.background,
            filled_element_disabled: system_color.transparent,
            ghost_element: system_color.transparent,
            ghost_element_hover: theme.lowest.base.default.background,
            ghost_element_active: theme.lowest.base.hovered.background,
            ghost_element_selected: theme.lowest.accent.default.background,
            ghost_element_disabled: system_color.transparent,
        }
    }
}

#[derive(Default, PartialEq, EnumIter, Clone, Copy)]
pub enum HighlightColor {
    #[default]
    Default,
    Comment,
    String,
    Function,
    Keyword,
}

impl HighlightColor {
    pub fn hsla(&self, theme: &Theme) -> Hsla {
        let system_color = SystemColor::new();

        match self {
            Self::Default => theme
                .syntax
                .get("primary")
                .cloned()
                .unwrap_or_else(|| rgb::<Hsla>(0xff00ff)),
            Self::Comment => theme
                .syntax
                .get("comment")
                .cloned()
                .unwrap_or_else(|| rgb::<Hsla>(0xff00ff)),
            Self::String => theme
                .syntax
                .get("string")
                .cloned()
                .unwrap_or_else(|| rgb::<Hsla>(0xff00ff)),
            Self::Function => theme
                .syntax
                .get("function")
                .cloned()
                .unwrap_or_else(|| rgb::<Hsla>(0xff00ff)),
            Self::Keyword => theme
                .syntax
                .get("keyword")
                .cloned()
                .unwrap_or_else(|| rgb::<Hsla>(0xff00ff)),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum FileSystemStatus {
    #[default]
    None,
    Conflict,
    Deleted,
}

impl FileSystemStatus {
    pub fn to_string(&self) -> String {
        match self {
            Self::None => "None".to_string(),
            Self::Conflict => "Conflict".to_string(),
            Self::Deleted => "Deleted".to_string(),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum GitStatus {
    #[default]
    None,
    Created,
    Modified,
    Deleted,
    Conflict,
    Renamed,
}

impl GitStatus {
    pub fn to_string(&self) -> String {
        match self {
            Self::None => "None".to_string(),
            Self::Created => "Created".to_string(),
            Self::Modified => "Modified".to_string(),
            Self::Deleted => "Deleted".to_string(),
            Self::Conflict => "Conflict".to_string(),
            Self::Renamed => "Renamed".to_string(),
        }
    }

    pub fn hsla(&self, cx: &WindowContext) -> Hsla {
        let theme = theme(cx);
        let system_color = SystemColor::new();

        match self {
            Self::None => system_color.transparent,
            Self::Created => theme.lowest.positive.default.foreground,
            Self::Modified => theme.lowest.warning.default.foreground,
            Self::Deleted => theme.lowest.negative.default.foreground,
            Self::Conflict => theme.lowest.warning.default.foreground,
            Self::Renamed => theme.lowest.accent.default.foreground,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum DiagnosticStatus {
    #[default]
    None,
    Error,
    Warning,
    Info,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum IconSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum OrderMethod {
    #[default]
    Ascending,
    Descending,
    MostRecent,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum Shape {
    #[default]
    Circle,
    RoundedRectangle,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, EnumIter)]
pub enum DisclosureControlVisibility {
    #[default]
    OnHover,
    Always,
}

#[derive(Default, PartialEq, Copy, Clone, EnumIter, strum::Display)]
pub enum InteractionState {
    #[default]
    Enabled,
    Hovered,
    Active,
    Focused,
    Disabled,
}

impl InteractionState {
    pub fn if_enabled(&self, enabled: bool) -> Self {
        if enabled {
            *self
        } else {
            InteractionState::Disabled
        }
    }
}

#[derive(Default, PartialEq)]
pub enum SelectedState {
    #[default]
    Unselected,
    PartiallySelected,
    Selected,
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub enum Toggleable {
    Toggleable(ToggleState),
    #[default]
    NotToggleable,
}

impl Toggleable {
    pub fn is_toggled(&self) -> bool {
        match self {
            Self::Toggleable(ToggleState::Toggled) => true,
            _ => false,
        }
    }
}

impl From<ToggleState> for Toggleable {
    fn from(state: ToggleState) -> Self {
        Self::Toggleable(state)
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ToggleState {
    /// The "on" state of a toggleable element.
    ///
    /// Example:
    ///     - A collasable list that is currently expanded
    ///     - A toggle button that is currently on.
    Toggled,
    /// The "off" state of a toggleable element.
    ///
    /// Example:
    ///     - A collasable list that is currently collapsed
    ///     - A toggle button that is currently off.
    #[default]
    NotToggled,
}

impl From<Toggleable> for ToggleState {
    fn from(toggleable: Toggleable) -> Self {
        match toggleable {
            Toggleable::Toggleable(state) => state,
            Toggleable::NotToggleable => ToggleState::NotToggled,
        }
    }
}

impl From<bool> for ToggleState {
    fn from(toggled: bool) -> Self {
        if toggled {
            ToggleState::Toggled
        } else {
            ToggleState::NotToggled
        }
    }
}
