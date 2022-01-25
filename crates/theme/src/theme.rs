mod resolution;
mod theme_registry;

use gpui::{
    color::Color,
    elements::{ContainerStyle, ImageStyle, LabelStyle},
    fonts::{HighlightStyle, TextStyle},
    Border,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};

pub use theme_registry::*;

pub const DEFAULT_THEME_NAME: &'static str = "black";

#[derive(Deserialize, Default)]
pub struct Theme {
    #[serde(default)]
    pub name: String,
    pub workspace: Workspace,
    pub chat_panel: ChatPanel,
    pub contacts_panel: ContactsPanel,
    pub project_panel: ProjectPanel,
    pub selector: Selector,
    pub editor: EditorStyle,
    pub project_diagnostics: ProjectDiagnostics,
}

#[derive(Deserialize, Default)]
pub struct Workspace {
    pub background: Color,
    pub titlebar: Titlebar,
    pub tab: Tab,
    pub active_tab: Tab,
    pub pane_divider: Border,
    pub left_sidebar: Sidebar,
    pub right_sidebar: Sidebar,
    pub status_bar: StatusBar,
}

#[derive(Clone, Deserialize, Default)]
pub struct Titlebar {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub height: f32,
    pub title: TextStyle,
    pub avatar_width: f32,
    pub avatar_ribbon: AvatarRibbon,
    pub offline_icon: OfflineIcon,
    pub share_icon_color: Color,
    pub share_icon_active_color: Color,
    pub avatar: ImageStyle,
    pub sign_in_prompt: ContainedText,
    pub hovered_sign_in_prompt: ContainedText,
    pub outdated_warning: ContainedText,
}

#[derive(Clone, Deserialize, Default)]
pub struct AvatarRibbon {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Deserialize, Default)]
pub struct OfflineIcon {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub width: f32,
    pub color: Color,
}

#[derive(Clone, Deserialize, Default)]
pub struct Tab {
    pub height: f32,
    #[serde(flatten)]
    pub container: ContainerStyle,
    #[serde(flatten)]
    pub label: LabelStyle,
    pub spacing: f32,
    pub icon_width: f32,
    pub icon_close: Color,
    pub icon_close_active: Color,
    pub icon_dirty: Color,
    pub icon_conflict: Color,
}

#[derive(Deserialize, Default)]
pub struct Sidebar {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub width: f32,
    pub item: SidebarItem,
    pub active_item: SidebarItem,
    pub resize_handle: ContainerStyle,
}

#[derive(Deserialize, Default)]
pub struct SidebarItem {
    pub icon_color: Color,
    pub icon_size: f32,
    pub height: f32,
}

#[derive(Deserialize, Default)]
pub struct StatusBar {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub height: f32,
    pub cursor_position: TextStyle,
    pub diagnostic_icon_size: f32,
    pub diagnostic_icon_spacing: f32,
    pub diagnostic_icon_color: Color,
    pub diagnostic_message: TextStyle,
}

#[derive(Deserialize, Default)]
pub struct ChatPanel {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub message: ChatMessage,
    pub pending_message: ChatMessage,
    pub channel_select: ChannelSelect,
    pub input_editor: InputEditorStyle,
    pub sign_in_prompt: TextStyle,
    pub hovered_sign_in_prompt: TextStyle,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProjectPanel {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub entry: ProjectPanelEntry,
    pub hovered_entry: ProjectPanelEntry,
    pub selected_entry: ProjectPanelEntry,
    pub hovered_selected_entry: ProjectPanelEntry,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProjectPanelEntry {
    pub height: f32,
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub text: TextStyle,
    pub icon_color: Color,
    pub icon_size: f32,
    pub icon_spacing: f32,
}

#[derive(Deserialize, Default)]
pub struct ContactsPanel {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub host_row_height: f32,
    pub host_avatar: ImageStyle,
    pub host_username: ContainedText,
    pub tree_branch_width: f32,
    pub tree_branch_color: Color,
    pub shared_project: WorktreeRow,
    pub hovered_shared_project: WorktreeRow,
    pub unshared_project: WorktreeRow,
    pub hovered_unshared_project: WorktreeRow,
}

#[derive(Deserialize, Default)]
pub struct WorktreeRow {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub height: f32,
    pub name: ContainedText,
    pub guest_avatar: ImageStyle,
    pub guest_avatar_spacing: f32,
}

#[derive(Deserialize, Default)]
pub struct ChatMessage {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub body: TextStyle,
    pub sender: ContainedText,
    pub timestamp: ContainedText,
}

#[derive(Deserialize, Default)]
pub struct ChannelSelect {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub header: ChannelName,
    pub item: ChannelName,
    pub active_item: ChannelName,
    pub hovered_item: ChannelName,
    pub hovered_active_item: ChannelName,
    pub menu: ContainerStyle,
}

#[derive(Deserialize, Default)]
pub struct ChannelName {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub hash: ContainedText,
    pub name: TextStyle,
}

#[derive(Deserialize, Default)]
pub struct Selector {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub empty: ContainedLabel,
    pub input_editor: InputEditorStyle,
    pub item: ContainedLabel,
    pub active_item: ContainedLabel,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct ContainedText {
    #[serde(flatten)]
    pub container: ContainerStyle,
    #[serde(flatten)]
    pub text: TextStyle,
}

#[derive(Clone, Deserialize, Default)]
pub struct ContainedLabel {
    #[serde(flatten)]
    pub container: ContainerStyle,
    #[serde(flatten)]
    pub label: LabelStyle,
}

#[derive(Clone, Deserialize, Default)]
pub struct ProjectDiagnostics {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub empty_message: TextStyle,
    pub status_bar_item: ContainedText,
}

#[derive(Clone, Deserialize, Default)]
pub struct EditorStyle {
    pub text: TextStyle,
    #[serde(default)]
    pub placeholder_text: Option<TextStyle>,
    pub background: Color,
    pub selection: SelectionStyle,
    pub gutter_background: Color,
    pub active_line_background: Color,
    pub highlighted_line_background: Color,
    pub line_number: Color,
    pub line_number_active: Color,
    pub guest_selections: Vec<SelectionStyle>,
    pub syntax: Arc<SyntaxTheme>,
    pub diagnostic_path_header: DiagnosticPathHeader,
    pub diagnostic_header: DiagnosticHeader,
    pub error_diagnostic: DiagnosticStyle,
    pub invalid_error_diagnostic: DiagnosticStyle,
    pub warning_diagnostic: DiagnosticStyle,
    pub invalid_warning_diagnostic: DiagnosticStyle,
    pub information_diagnostic: DiagnosticStyle,
    pub invalid_information_diagnostic: DiagnosticStyle,
    pub hint_diagnostic: DiagnosticStyle,
    pub invalid_hint_diagnostic: DiagnosticStyle,
}

#[derive(Clone, Deserialize, Default)]
pub struct DiagnosticPathHeader {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub filename: ContainedText,
    pub path: ContainedText,
}

#[derive(Clone, Deserialize, Default)]
pub struct DiagnosticHeader {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub message: ContainedLabel,
    pub code: ContainedText,
    pub icon: DiagnosticHeaderIcon,
}

#[derive(Clone, Deserialize, Default)]
pub struct DiagnosticHeaderIcon {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub width: f32,
}

#[derive(Clone, Deserialize, Default)]
pub struct DiagnosticStyle {
    pub message: LabelStyle,
    #[serde(default)]
    pub header: ContainerStyle,
}

#[derive(Clone, Copy, Default, Deserialize)]
pub struct SelectionStyle {
    pub cursor: Color,
    pub selection: Color,
}

#[derive(Clone, Deserialize, Default)]
pub struct InputEditorStyle {
    #[serde(flatten)]
    pub container: ContainerStyle,
    pub text: TextStyle,
    #[serde(default)]
    pub placeholder_text: Option<TextStyle>,
    pub selection: SelectionStyle,
}

impl EditorStyle {
    pub fn placeholder_text(&self) -> &TextStyle {
        self.placeholder_text.as_ref().unwrap_or(&self.text)
    }

    pub fn replica_selection_style(&self, replica_id: u16) -> &SelectionStyle {
        let style_ix = replica_id as usize % (self.guest_selections.len() + 1);
        if style_ix == 0 {
            &self.selection
        } else {
            &self.guest_selections[style_ix - 1]
        }
    }
}

impl InputEditorStyle {
    pub fn as_editor(&self) -> EditorStyle {
        let default_diagnostic_style = DiagnosticStyle {
            message: self.text.clone().into(),
            header: Default::default(),
        };
        EditorStyle {
            text: self.text.clone(),
            placeholder_text: self.placeholder_text.clone(),
            background: self
                .container
                .background_color
                .unwrap_or(Color::transparent_black()),
            selection: self.selection,
            gutter_background: Default::default(),
            active_line_background: Default::default(),
            highlighted_line_background: Default::default(),
            line_number: Default::default(),
            line_number_active: Default::default(),
            guest_selections: Default::default(),
            syntax: Default::default(),
            diagnostic_path_header: DiagnosticPathHeader {
                container: Default::default(),
                filename: ContainedText {
                    container: Default::default(),
                    text: self.text.clone(),
                },
                path: ContainedText {
                    container: Default::default(),
                    text: self.text.clone(),
                },
            },
            diagnostic_header: DiagnosticHeader {
                container: Default::default(),
                message: ContainedLabel {
                    container: Default::default(),
                    label: self.text.clone().into(),
                },
                code: ContainedText {
                    container: Default::default(),
                    text: self.text.clone(),
                },
                icon: Default::default(),
            },
            error_diagnostic: default_diagnostic_style.clone(),
            invalid_error_diagnostic: default_diagnostic_style.clone(),
            warning_diagnostic: default_diagnostic_style.clone(),
            invalid_warning_diagnostic: default_diagnostic_style.clone(),
            information_diagnostic: default_diagnostic_style.clone(),
            invalid_information_diagnostic: default_diagnostic_style.clone(),
            hint_diagnostic: default_diagnostic_style.clone(),
            invalid_hint_diagnostic: default_diagnostic_style.clone(),
        }
    }
}

#[derive(Default)]
pub struct SyntaxTheme {
    pub highlights: Vec<(String, HighlightStyle)>,
}

impl SyntaxTheme {
    pub fn new(highlights: Vec<(String, HighlightStyle)>) -> Self {
        Self { highlights }
    }
}

impl<'de> Deserialize<'de> for SyntaxTheme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let syntax_data: HashMap<String, HighlightStyle> = Deserialize::deserialize(deserializer)?;

        let mut result = Self::new(Vec::new());
        for (key, style) in syntax_data {
            match result
                .highlights
                .binary_search_by(|(needle, _)| needle.cmp(&key))
            {
                Ok(i) | Err(i) => {
                    result.highlights.insert(i, (key, style));
                }
            }
        }

        Ok(result)
    }
}
