// This file was generated by the `theme_importer`.
// Be careful when modifying it by hand.

use gpui::rgba;

use crate::{
    Appearance, ThemeColorsRefinement, UserTheme, UserThemeFamily, UserThemeStylesRefinement,
};

pub fn gruvbox() -> UserThemeFamily {
    UserThemeFamily {
        name: "Gruvbox".into(),
        author: "morhetz".into(),
        themes: vec![
            UserTheme {
                name: "Gruvbox Dark Hard".into(),
                appearance: Appearance::Dark,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0x3c3836ff).into()),
                        border_variant: Some(rgba(0x3c3836ff).into()),
                        border_focused: Some(rgba(0x3c3836ff).into()),
                        border_selected: Some(rgba(0x3c3836ff).into()),
                        border_transparent: Some(rgba(0x3c3836ff).into()),
                        border_disabled: Some(rgba(0x3c3836ff).into()),
                        background: Some(rgba(0x1d2021ff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0xebdbb2ff).into()),
                        tab_inactive_background: Some(rgba(0x1d2021ff).into()),
                        tab_active_background: Some(rgba(0x32302fff).into()),
                        terminal_background: Some(rgba(0x1d2021ff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0xfb4833ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0xb8bb25ff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xfabd2eff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x83a598ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0xd3869bff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x8ec07cff).into()),
                        terminal_ansi_bright_white: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_black: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0xa89984ff).into()),
                        ..Default::default()
                    },
                },
            },
            UserTheme {
                name: "Gruvbox Dark Medium".into(),
                appearance: Appearance::Dark,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0x3c3836ff).into()),
                        border_variant: Some(rgba(0x3c3836ff).into()),
                        border_focused: Some(rgba(0x3c3836ff).into()),
                        border_selected: Some(rgba(0x3c3836ff).into()),
                        border_transparent: Some(rgba(0x3c3836ff).into()),
                        border_disabled: Some(rgba(0x3c3836ff).into()),
                        background: Some(rgba(0x282828ff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0xebdbb2ff).into()),
                        tab_inactive_background: Some(rgba(0x282828ff).into()),
                        tab_active_background: Some(rgba(0x3c3836ff).into()),
                        terminal_background: Some(rgba(0x282828ff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0xfb4833ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0xb8bb25ff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xfabd2eff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x83a598ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0xd3869bff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x8ec07cff).into()),
                        terminal_ansi_bright_white: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_black: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0xa89984ff).into()),
                        ..Default::default()
                    },
                },
            },
            UserTheme {
                name: "Gruvbox Dark Soft".into(),
                appearance: Appearance::Dark,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0x3c3836ff).into()),
                        border_variant: Some(rgba(0x3c3836ff).into()),
                        border_focused: Some(rgba(0x3c3836ff).into()),
                        border_selected: Some(rgba(0x3c3836ff).into()),
                        border_transparent: Some(rgba(0x3c3836ff).into()),
                        border_disabled: Some(rgba(0x3c3836ff).into()),
                        background: Some(rgba(0x32302fff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0xebdbb2ff).into()),
                        tab_inactive_background: Some(rgba(0x32302fff).into()),
                        tab_active_background: Some(rgba(0x504945ff).into()),
                        terminal_background: Some(rgba(0x32302fff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0xfb4833ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0xb8bb25ff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xfabd2eff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x83a598ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0xd3869bff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x8ec07cff).into()),
                        terminal_ansi_bright_white: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_black: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0xa89984ff).into()),
                        ..Default::default()
                    },
                },
            },
            UserTheme {
                name: "Gruvbox Light Hard".into(),
                appearance: Appearance::Light,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0xebdbb2ff).into()),
                        border_variant: Some(rgba(0xebdbb2ff).into()),
                        border_focused: Some(rgba(0xebdbb2ff).into()),
                        border_selected: Some(rgba(0xebdbb2ff).into()),
                        border_transparent: Some(rgba(0xebdbb2ff).into()),
                        border_disabled: Some(rgba(0xebdbb2ff).into()),
                        background: Some(rgba(0xf9f5d7ff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0x3c3836ff).into()),
                        tab_inactive_background: Some(rgba(0xf9f5d7ff).into()),
                        tab_active_background: Some(rgba(0xf2e5bcff).into()),
                        terminal_background: Some(rgba(0xf9f5d7ff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0x9d0006ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0x79740eff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xb57613ff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x066578ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0x8f3e71ff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x427b58ff).into()),
                        terminal_ansi_bright_white: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_black: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0x7c6f64ff).into()),
                        ..Default::default()
                    },
                },
            },
            UserTheme {
                name: "Gruvbox Light Medium".into(),
                appearance: Appearance::Light,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0xebdbb2ff).into()),
                        border_variant: Some(rgba(0xebdbb2ff).into()),
                        border_focused: Some(rgba(0xebdbb2ff).into()),
                        border_selected: Some(rgba(0xebdbb2ff).into()),
                        border_transparent: Some(rgba(0xebdbb2ff).into()),
                        border_disabled: Some(rgba(0xebdbb2ff).into()),
                        background: Some(rgba(0xfbf1c7ff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0x3c3836ff).into()),
                        tab_inactive_background: Some(rgba(0xfbf1c7ff).into()),
                        tab_active_background: Some(rgba(0xebdbb2ff).into()),
                        terminal_background: Some(rgba(0xfbf1c7ff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0x9d0006ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0x79740eff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xb57613ff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x066578ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0x8f3e71ff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x427b58ff).into()),
                        terminal_ansi_bright_white: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_black: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0x7c6f64ff).into()),
                        ..Default::default()
                    },
                },
            },
            UserTheme {
                name: "Gruvbox Light Soft".into(),
                appearance: Appearance::Light,
                styles: UserThemeStylesRefinement {
                    colors: ThemeColorsRefinement {
                        border: Some(rgba(0xebdbb2ff).into()),
                        border_variant: Some(rgba(0xebdbb2ff).into()),
                        border_focused: Some(rgba(0xebdbb2ff).into()),
                        border_selected: Some(rgba(0xebdbb2ff).into()),
                        border_transparent: Some(rgba(0xebdbb2ff).into()),
                        border_disabled: Some(rgba(0xebdbb2ff).into()),
                        background: Some(rgba(0xf2e5bcff).into()),
                        element_background: Some(rgba(0x44858780).into()),
                        text: Some(rgba(0x3c3836ff).into()),
                        tab_inactive_background: Some(rgba(0xf2e5bcff).into()),
                        tab_active_background: Some(rgba(0xd5c4a1ff).into()),
                        terminal_background: Some(rgba(0xf2e5bcff).into()),
                        terminal_ansi_bright_black: Some(rgba(0x928374ff).into()),
                        terminal_ansi_bright_red: Some(rgba(0x9d0006ff).into()),
                        terminal_ansi_bright_green: Some(rgba(0x79740eff).into()),
                        terminal_ansi_bright_yellow: Some(rgba(0xb57613ff).into()),
                        terminal_ansi_bright_blue: Some(rgba(0x066578ff).into()),
                        terminal_ansi_bright_magenta: Some(rgba(0x8f3e71ff).into()),
                        terminal_ansi_bright_cyan: Some(rgba(0x427b58ff).into()),
                        terminal_ansi_bright_white: Some(rgba(0x3c3836ff).into()),
                        terminal_ansi_black: Some(rgba(0xebdbb2ff).into()),
                        terminal_ansi_red: Some(rgba(0xcc241cff).into()),
                        terminal_ansi_green: Some(rgba(0x989719ff).into()),
                        terminal_ansi_yellow: Some(rgba(0xd79920ff).into()),
                        terminal_ansi_blue: Some(rgba(0x448587ff).into()),
                        terminal_ansi_magenta: Some(rgba(0xb16185ff).into()),
                        terminal_ansi_cyan: Some(rgba(0x679d6aff).into()),
                        terminal_ansi_white: Some(rgba(0x7c6f64ff).into()),
                        ..Default::default()
                    },
                },
            },
        ],
    }
}
