use collections::HashMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::Setting;
use std::sync::Arc;

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSettings {
    #[serde(default)]
    pub lsp: HashMap<Arc<str>, LspSettings>,
    #[serde(default)]
    pub git: GitSettings,
    // TODO kb better names and docs and tests
    // TODO kb how to react on their changes?
    #[serde(default)]
    pub scan_exclude_files: Vec<String>,
    #[serde(default)]
    pub scan_include_files: Vec<String>,
}

#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GitSettings {
    pub git_gutter: Option<GitGutterSetting>,
    pub gutter_debounce: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitGutterSetting {
    #[default]
    TrackedFiles,
    Hide,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct LspSettings {
    pub initialization_options: Option<serde_json::Value>,
}

impl Setting for ProjectSettings {
    const KEY: Option<&'static str> = None;

    type FileContent = Self;

    fn load(
        default_value: &Self::FileContent,
        user_values: &[&Self::FileContent],
        _: &gpui::AppContext,
    ) -> anyhow::Result<Self> {
        Self::load_via_json_merge(default_value, user_values)
    }
}
