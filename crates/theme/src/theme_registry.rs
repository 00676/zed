use crate::Theme;
use anyhow::{Context, Result};
use gpui::{fonts, AssetSource, FontCache};
use parking_lot::Mutex;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

pub struct ThemeRegistry {
    assets: Box<dyn AssetSource>,
    themes: Mutex<HashMap<String, Arc<Theme>>>,
    theme_data: Mutex<HashMap<String, Arc<Value>>>,
    font_cache: Arc<FontCache>,
}

impl ThemeRegistry {
    pub fn new(source: impl AssetSource, font_cache: Arc<FontCache>) -> Arc<Self> {
        Arc::new(Self {
            assets: Box::new(source),
            themes: Default::default(),
            theme_data: Default::default(),
            font_cache,
        })
    }

    pub fn list(&self) -> impl Iterator<Item = String> {
        self.assets.list("themes/").into_iter().filter_map(|path| {
            let filename = path.strip_prefix("themes/")?;
            let theme_name = filename.strip_suffix(".json")?;
            if theme_name.starts_with('_') {
                None
            } else {
                Some(theme_name.to_string())
            }
        })
    }

    pub fn clear(&self) {
        self.theme_data.lock().clear();
        self.themes.lock().clear();
    }

    pub fn get(&self, name: &str) -> Result<Arc<Theme>> {
        if let Some(theme) = self.themes.lock().get(name) {
            return Ok(theme.clone());
        }

        let asset_path = format!("themes/{}.json", name);
        let source_code = self
            .assets
            .load(&asset_path)
            .with_context(|| format!("failed to load theme file {}", asset_path))?;
        let mut theme: Theme = fonts::with_font_cache(self.font_cache.clone(), || {
            serde_path_to_error::deserialize(&mut serde_json::Deserializer::from_slice(
                source_code.as_ref(),
            ))
        })?;

        theme.name = name.into();
        let theme = Arc::new(theme);
        self.themes.lock().insert(name.to_string(), theme.clone());
        Ok(theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use gpui::MutableAppContext;

    #[gpui::test]
    fn test_theme_extension(cx: &mut MutableAppContext) {
        let assets = TestAssets(&[
            (
                "themes/_base.toml",
                r##"
                [ui.active_tab]
                extends = "$ui.tab"
                border.color = "#666666"
                text = "$text_colors.bright"

                [ui.tab]
                extends = "$ui.element"
                text = "$text_colors.dull"

                [ui.element]
                background = "#111111"
                border = {width = 2.0, color = "#00000000"}

                [editor]
                background = "#222222"
                default_text = "$text_colors.regular"
                "##,
            ),
            (
                "themes/light.toml",
                r##"
                extends = "_base"

                [text_colors]
                bright = "#ffffff"
                regular = "#eeeeee"
                dull = "#dddddd"

                [editor]
                background = "#232323"
                "##,
            ),
        ]);

        let registry = ThemeRegistry::new(assets, cx.font_cache().clone());
        let theme_data = registry.load("light").unwrap();

        assert_eq!(
            theme_data.as_ref(),
            &serde_json::json!({
              "ui": {
                "active_tab": {
                  "background": "#111111",
                  "border": {
                    "width": 2.0,
                    "color": "#666666"
                  },
                  "extends": "$ui.tab",
                  "text": "#ffffff"
                },
                "tab": {
                  "background": "#111111",
                  "border": {
                    "width": 2.0,
                    "color": "#00000000"
                  },
                  "extends": "$ui.element",
                  "text": "#dddddd"
                },
                "element": {
                  "background": "#111111",
                  "border": {
                    "width": 2.0,
                    "color": "#00000000"
                  }
                }
              },
              "editor": {
                "background": "#232323",
                "default_text": "#eeeeee"
              },
              "extends": "_base",
              "text_colors": {
                "bright": "#ffffff",
                "regular": "#eeeeee",
                "dull": "#dddddd"
              }
            })
        );
    }

    #[gpui::test]
    fn test_nested_extension(cx: &mut MutableAppContext) {
        let assets = TestAssets(&[(
            "themes/theme.toml",
            r##"
                [a]
                text = { extends = "$text.0" }

                [b]
                extends = "$a"
                text = { extends = "$text.1" }

                [text]
                0 = { color = "red" }
                1 = { color = "blue" }
            "##,
        )]);

        let registry = ThemeRegistry::new(assets, cx.font_cache().clone());
        let theme_data = registry.load("theme").unwrap();
        assert_eq!(
            theme_data
                .get("b")
                .unwrap()
                .get("text")
                .unwrap()
                .get("color")
                .unwrap(),
            "blue"
        );
    }

    struct TestAssets(&'static [(&'static str, &'static str)]);

    impl AssetSource for TestAssets {
        fn load(&self, path: &str) -> Result<std::borrow::Cow<[u8]>> {
            if let Some(row) = self.0.iter().find(|e| e.0 == path) {
                Ok(row.1.as_bytes().into())
            } else {
                Err(anyhow!("no such path {}", path))
            }
        }

        fn list(&self, prefix: &str) -> Vec<std::borrow::Cow<'static, str>> {
            self.0
                .iter()
                .copied()
                .filter_map(|(path, _)| {
                    if path.starts_with(prefix) {
                        Some(path.into())
                    } else {
                        None
                    }
                })
                .collect()
        }
    }
}
