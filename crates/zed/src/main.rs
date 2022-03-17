// Allow binary to be called Zed for a nice application menu when running executable direcly
#![allow(non_snake_case)]

use anyhow::{anyhow, Context, Result};
use client::{self, http, ChannelList, UserStore};
use fs::OpenOptions;
use futures::{channel::oneshot, StreamExt};
use gpui::{App, AssetSource, Task};
use log::LevelFilter;
use parking_lot::Mutex;
use project::Fs;
use simplelog::SimpleLogger;
use smol::process::Command;
use std::{env, fs, path::PathBuf, sync::Arc};
use theme::{ThemeRegistry, DEFAULT_THEME_NAME};
use util::ResultExt;
use workspace::{
    self,
    settings::{self, SettingsFile},
    AppState, OpenNew, OpenParams, OpenPaths, Settings,
};
use zed::{
    self, assets::Assets, build_window_options, build_workspace, fs::RealFs, language, menus,
};

fn main() {
    init_logger();

    let app = gpui::App::new(Assets).unwrap();
    load_embedded_fonts(&app);

    let fs = Arc::new(RealFs);
    let themes = ThemeRegistry::new(Assets, app.font_cache());
    let theme = themes.get(DEFAULT_THEME_NAME).unwrap();
    let default_settings = Settings::new("Zed Mono", &app.font_cache(), theme)
        .unwrap()
        .with_overrides(
            language::PLAIN_TEXT.name(),
            settings::LanguageOverride {
                soft_wrap: Some(settings::SoftWrap::PreferredLineLength),
                ..Default::default()
            },
        )
        .with_overrides(
            "Markdown",
            settings::LanguageOverride {
                soft_wrap: Some(settings::SoftWrap::PreferredLineLength),
                ..Default::default()
            },
        );
    let settings_file = load_settings_file(&app, fs.clone());

    let login_shell_env_loaded = if stdout_is_a_pty() {
        Task::ready(())
    } else {
        app.background().spawn(async {
            load_login_shell_environment().await.log_err();
        })
    };

    app.run(move |cx| {
        let http = http::client();
        let client = client::Client::new(http.clone());
        let mut languages = language::build_language_registry(login_shell_env_loaded);
        let user_store = cx.add_model(|cx| UserStore::new(client.clone(), http.clone(), cx));
        let channel_list =
            cx.add_model(|cx| ChannelList::new(user_store.clone(), client.clone(), cx));

        project::Project::init(&client);
        client::Channel::init(&client);
        client::init(client.clone(), cx);
        workspace::init(cx);
        editor::init(cx);
        go_to_line::init(cx);
        file_finder::init(cx);
        chat_panel::init(cx);
        outline::init(cx);
        project_symbols::init(cx);
        project_panel::init(cx);
        diagnostics::init(cx);
        search::init(cx);
        cx.spawn({
            let client = client.clone();
            |cx| async move {
                if client.has_keychain_credentials(&cx) {
                    client.authenticate_and_connect(&cx).await?;
                }
                Ok::<_, anyhow::Error>(())
            }
        })
        .detach_and_log_err(cx);

        let settings_file = cx.background().block(settings_file).unwrap();
        let mut settings_rx = Settings::from_files(
            default_settings,
            vec![settings_file],
            themes.clone(),
            cx.font_cache().clone(),
        );
        let settings = cx.background().block(settings_rx.next()).unwrap();
        cx.spawn(|mut cx| async move {
            while let Some(settings) = settings_rx.next().await {
                cx.update(|cx| {
                    cx.update_global(|s, _| *s = settings);
                    cx.refresh_windows();
                });
            }
        })
        .detach();

        languages.set_language_server_download_dir(zed::ROOT_PATH.clone());
        languages.set_theme(&settings.theme.editor.syntax);
        cx.set_global(settings);

        let app_state = Arc::new(AppState {
            languages: Arc::new(languages),
            themes,
            channel_list,
            client,
            user_store,
            fs,
            build_window_options: &build_window_options,
            build_workspace: &build_workspace,
        });
        journal::init(app_state.clone(), cx);
        zed::init(&app_state, cx);
        theme_selector::init(app_state.themes.clone(), cx);

        cx.set_menus(menus::menus(&app_state.clone()));

        if stdout_is_a_pty() {
            cx.platform().activate(true);
        }

        let paths = collect_path_args();
        if paths.is_empty() {
            cx.dispatch_global_action(OpenNew(app_state.clone()));
        } else {
            cx.dispatch_global_action(OpenPaths(OpenParams { paths, app_state }));
        }
    });
}

fn init_logger() {
    let level = LevelFilter::Info;

    if stdout_is_a_pty() {
        SimpleLogger::init(level, Default::default()).expect("could not initialize logger");
    } else {
        let log_dir_path = dirs::home_dir()
            .expect("could not locate home directory for logging")
            .join("Library/Logs/");
        let log_file_path = log_dir_path.join("Zed.log");
        fs::create_dir_all(&log_dir_path).expect("could not create log directory");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path)
            .expect("could not open logfile");
        simplelog::WriteLogger::init(level, simplelog::Config::default(), log_file)
            .expect("could not initialize logger");
        log_panics::init();
    }
}

async fn load_login_shell_environment() -> Result<()> {
    let marker = "ZED_LOGIN_SHELL_START";
    let shell = env::var("SHELL").context(
        "SHELL environment variable is not assigned so we can't source login environment variables",
    )?;
    let output = Command::new(&shell)
        .args(["-lic", &format!("echo {marker} && /usr/bin/env")])
        .output()
        .await
        .context("failed to spawn login shell to source login environment variables")?;
    if !output.status.success() {
        Err(anyhow!("login shell exited with error"))?;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if let Some(env_output_start) = stdout.find(marker) {
        let env_output = &stdout[env_output_start + marker.len()..];
        for line in env_output.lines() {
            if let Some(separator_index) = line.find('=') {
                let key = &line[..separator_index];
                let value = &line[separator_index + 1..];
                env::set_var(key, value);
            }
        }
        log::info!(
            "set environment variables from shell:{}, path:{}",
            shell,
            env::var("PATH").unwrap_or_default(),
        );
    }

    Ok(())
}

fn stdout_is_a_pty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO as i32) != 0 }
}

fn collect_path_args() -> Vec<PathBuf> {
    env::args()
        .skip(1)
        .filter_map(|arg| match fs::canonicalize(arg) {
            Ok(path) => Some(path),
            Err(error) => {
                log::error!("error parsing path argument: {}", error);
                None
            }
        })
        .collect::<Vec<_>>()
}

fn load_embedded_fonts(app: &App) {
    let font_paths = Assets.list("fonts");
    let embedded_fonts = Mutex::new(Vec::new());
    smol::block_on(app.background().scoped(|scope| {
        for font_path in &font_paths {
            scope.spawn(async {
                let font_path = &*font_path;
                let font_bytes = Assets.load(&font_path).unwrap().to_vec();
                embedded_fonts.lock().push(Arc::from(font_bytes));
            });
        }
    }));
    app.platform()
        .fonts()
        .add_fonts(&embedded_fonts.into_inner())
        .unwrap();
}

fn load_settings_file(app: &App, fs: Arc<dyn Fs>) -> oneshot::Receiver<SettingsFile> {
    let executor = app.background();
    let (tx, rx) = oneshot::channel();
    executor
        .clone()
        .spawn(async move {
            let file = SettingsFile::new(fs, &executor, zed::SETTINGS_PATH.clone()).await;
            tx.send(file).ok()
        })
        .detach();
    rx
}
