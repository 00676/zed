mod request;
mod sign_in;

use anyhow::{anyhow, Result};
use client::Client;
use futures::{future::Shared, FutureExt, TryFutureExt};
use gpui::{actions, AppContext, Entity, ModelContext, ModelHandle, MutableAppContext, Task};
use language::{point_from_lsp, point_to_lsp, Anchor, Bias, Buffer, BufferSnapshot, ToPointUtf16};
use lsp::LanguageServer;
use node_runtime::NodeRuntime;
use settings::Settings;
use smol::{fs, stream::StreamExt};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};
use util::{fs::remove_matching, http::HttpClient, paths, ResultExt};

actions!(copilot_auth, [SignIn, SignOut]);

const COPILOT_NAMESPACE: &'static str = "copilot";
actions!(copilot, [NextSuggestion]);

pub fn init(client: Arc<Client>, node_runtime: Arc<NodeRuntime>, cx: &mut MutableAppContext) {
    let copilot = cx.add_model(|cx| Copilot::start(client.http_client(), node_runtime, cx));
    cx.set_global(copilot.clone());
    cx.add_global_action(|_: &SignIn, cx| {
        let copilot = Copilot::global(cx).unwrap();
        copilot
            .update(cx, |copilot, cx| copilot.sign_in(cx))
            .detach_and_log_err(cx);
    });
    cx.add_global_action(|_: &SignOut, cx| {
        let copilot = Copilot::global(cx).unwrap();
        copilot
            .update(cx, |copilot, cx| copilot.sign_out(cx))
            .detach_and_log_err(cx);
    });

    cx.observe(&copilot, |handle, cx| {
        let status = handle.read(cx).status();
        cx.update_global::<collections::CommandPaletteFilter, _, _>(
            move |filter, _cx| match status {
                Status::Authorized => filter.filtered_namespaces.remove(COPILOT_NAMESPACE),
                _ => filter.filtered_namespaces.insert(COPILOT_NAMESPACE),
            },
        );
    })
    .detach();

    sign_in::init(cx);
}

enum CopilotServer {
    Downloading,
    Error(Arc<str>),
    Started {
        server: Arc<LanguageServer>,
        status: SignInStatus,
    },
}

#[derive(Clone, Debug)]
enum SignInStatus {
    Authorized {
        _user: String,
    },
    Unauthorized {
        _user: String,
    },
    SigningIn {
        prompt: Option<request::PromptUserDeviceFlow>,
        task: Shared<Task<Result<(), Arc<anyhow::Error>>>>,
    },
    SignedOut,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Downloading,
    Error(Arc<str>),
    SignedOut,
    SigningIn {
        prompt: Option<request::PromptUserDeviceFlow>,
    },
    Unauthorized,
    Authorized,
}

impl Status {
    pub fn is_authorized(&self) -> bool {
        matches!(self, Status::Authorized)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    pub position: Anchor,
    pub text: String,
}

pub struct Copilot {
    server: CopilotServer,
}

impl Entity for Copilot {
    type Event = ();
}

impl Copilot {
    pub fn global(cx: &AppContext) -> Option<ModelHandle<Self>> {
        if cx.has_global::<ModelHandle<Self>>() {
            Some(cx.global::<ModelHandle<Self>>().clone())
        } else {
            None
        }
    }

    fn start(
        http: Arc<dyn HttpClient>,
        node_runtime: Arc<NodeRuntime>,
        cx: &mut ModelContext<Self>,
    ) -> Self {
        // TODO: Don't eagerly download the LSP
        cx.spawn(|this, mut cx| async move {
            let start_language_server = async {
                let server_path = get_copilot_lsp(http, node_runtime.clone()).await?;
                let node_path = node_runtime.binary_path().await?;
                let arguments: &[OsString] = &[server_path.into(), "--stdio".into()];
                let server =
                    LanguageServer::new(0, &node_path, arguments, Path::new("/"), cx.clone())?;

                let server = server.initialize(Default::default()).await?;
                let status = server
                    .request::<request::CheckStatus>(request::CheckStatusParams {
                        local_checks_only: false,
                    })
                    .await?;
                anyhow::Ok((server, status))
            };

            let server = start_language_server.await;
            this.update(&mut cx, |this, cx| {
                cx.notify();
                match server {
                    Ok((server, status)) => {
                        this.server = CopilotServer::Started {
                            server,
                            status: SignInStatus::SignedOut,
                        };
                        this.update_sign_in_status(status, cx);
                    }
                    Err(error) => {
                        this.server = CopilotServer::Error(error.to_string().into());
                    }
                }
            })
        })
        .detach();

        Self {
            server: CopilotServer::Downloading,
        }
    }

    fn sign_in(&mut self, cx: &mut ModelContext<Self>) -> Task<Result<()>> {
        if let CopilotServer::Started { server, status } = &mut self.server {
            let task = match status {
                SignInStatus::Authorized { .. } | SignInStatus::Unauthorized { .. } => {
                    Task::ready(Ok(())).shared()
                }
                SignInStatus::SigningIn { task, .. } => {
                    cx.notify(); // To re-show the prompt, just in case.
                    task.clone()
                }
                SignInStatus::SignedOut => {
                    let server = server.clone();
                    let task = cx
                        .spawn(|this, mut cx| async move {
                            let sign_in = async {
                                let sign_in = server
                                    .request::<request::SignInInitiate>(
                                        request::SignInInitiateParams {},
                                    )
                                    .await?;
                                match sign_in {
                                    request::SignInInitiateResult::AlreadySignedIn { user } => {
                                        Ok(request::SignInStatus::Ok { user })
                                    }
                                    request::SignInInitiateResult::PromptUserDeviceFlow(flow) => {
                                        this.update(&mut cx, |this, cx| {
                                            if let CopilotServer::Started { status, .. } =
                                                &mut this.server
                                            {
                                                if let SignInStatus::SigningIn {
                                                    prompt: prompt_flow,
                                                    ..
                                                } = status
                                                {
                                                    *prompt_flow = Some(flow.clone());
                                                    cx.notify();
                                                }
                                            }
                                        });
                                        let response = server
                                            .request::<request::SignInConfirm>(
                                                request::SignInConfirmParams {
                                                    user_code: flow.user_code,
                                                },
                                            )
                                            .await?;
                                        Ok(response)
                                    }
                                }
                            };

                            let sign_in = sign_in.await;
                            this.update(&mut cx, |this, cx| match sign_in {
                                Ok(status) => {
                                    this.update_sign_in_status(status, cx);
                                    Ok(())
                                }
                                Err(error) => {
                                    this.update_sign_in_status(
                                        request::SignInStatus::NotSignedIn,
                                        cx,
                                    );
                                    Err(Arc::new(error))
                                }
                            })
                        })
                        .shared();
                    *status = SignInStatus::SigningIn {
                        prompt: None,
                        task: task.clone(),
                    };
                    cx.notify();
                    task
                }
            };

            cx.foreground()
                .spawn(task.map_err(|err| anyhow!("{:?}", err)))
        } else {
            Task::ready(Err(anyhow!("copilot hasn't started yet")))
        }
    }

    fn sign_out(&mut self, cx: &mut ModelContext<Self>) -> Task<Result<()>> {
        if let CopilotServer::Started { server, status } = &mut self.server {
            *status = SignInStatus::SignedOut;
            cx.notify();

            let server = server.clone();
            cx.background().spawn(async move {
                server
                    .request::<request::SignOut>(request::SignOutParams {})
                    .await?;
                anyhow::Ok(())
            })
        } else {
            Task::ready(Err(anyhow!("copilot hasn't started yet")))
        }
    }

    pub fn completion<T>(
        &self,
        buffer: &ModelHandle<Buffer>,
        position: T,
        cx: &mut ModelContext<Self>,
    ) -> Task<Result<Option<Completion>>>
    where
        T: ToPointUtf16,
    {
        let server = match self.authorized_server() {
            Ok(server) => server,
            Err(error) => return Task::ready(Err(error)),
        };

        let buffer = buffer.read(cx).snapshot();
        let request = server
            .request::<request::GetCompletions>(build_completion_params(&buffer, position, cx));
        cx.background().spawn(async move {
            let result = request.await?;
            let completion = result
                .completions
                .into_iter()
                .next()
                .map(|completion| completion_from_lsp(completion, &buffer));
            anyhow::Ok(completion)
        })
    }

    pub fn completions_cycling<T>(
        &self,
        buffer: &ModelHandle<Buffer>,
        position: T,
        cx: &mut ModelContext<Self>,
    ) -> Task<Result<Vec<Completion>>>
    where
        T: ToPointUtf16,
    {
        let server = match self.authorized_server() {
            Ok(server) => server,
            Err(error) => return Task::ready(Err(error)),
        };

        let buffer = buffer.read(cx).snapshot();
        let request = server.request::<request::GetCompletionsCycling>(build_completion_params(
            &buffer, position, cx,
        ));
        cx.background().spawn(async move {
            let result = request.await?;
            let completions = result
                .completions
                .into_iter()
                .map(|completion| completion_from_lsp(completion, &buffer))
                .collect();
            anyhow::Ok(completions)
        })
    }

    pub fn status(&self) -> Status {
        match &self.server {
            CopilotServer::Downloading => Status::Downloading,
            CopilotServer::Error(error) => Status::Error(error.clone()),
            CopilotServer::Started { status, .. } => match status {
                SignInStatus::Authorized { .. } => Status::Authorized,
                SignInStatus::Unauthorized { .. } => Status::Unauthorized,
                SignInStatus::SigningIn { prompt, .. } => Status::SigningIn {
                    prompt: prompt.clone(),
                },
                SignInStatus::SignedOut => Status::SignedOut,
            },
        }
    }

    fn update_sign_in_status(
        &mut self,
        lsp_status: request::SignInStatus,
        cx: &mut ModelContext<Self>,
    ) {
        if let CopilotServer::Started { status, .. } = &mut self.server {
            *status = match lsp_status {
                request::SignInStatus::Ok { user } | request::SignInStatus::MaybeOk { user } => {
                    SignInStatus::Authorized { _user: user }
                }
                request::SignInStatus::NotAuthorized { user } => {
                    SignInStatus::Unauthorized { _user: user }
                }
                _ => SignInStatus::SignedOut,
            };
            cx.notify();
        }
    }

    fn authorized_server(&self) -> Result<Arc<LanguageServer>> {
        match &self.server {
            CopilotServer::Downloading => Err(anyhow!("copilot is still downloading")),
            CopilotServer::Error(error) => Err(anyhow!(
                "copilot was not started because of an error: {}",
                error
            )),
            CopilotServer::Started { server, status } => {
                if matches!(status, SignInStatus::Authorized { .. }) {
                    Ok(server.clone())
                } else {
                    Err(anyhow!("must sign in before using copilot"))
                }
            }
        }
    }
}

fn build_completion_params<T>(
    buffer: &BufferSnapshot,
    position: T,
    cx: &AppContext,
) -> request::GetCompletionsParams
where
    T: ToPointUtf16,
{
    let position = position.to_point_utf16(&buffer);
    let language_name = buffer.language_at(position).map(|language| language.name());
    let language_name = language_name.as_deref();

    let path;
    let relative_path;
    if let Some(file) = buffer.file() {
        if let Some(file) = file.as_local() {
            path = file.abs_path(cx);
        } else {
            path = file.full_path(cx);
        }
        relative_path = file.path().to_path_buf();
    } else {
        path = PathBuf::from("/untitled");
        relative_path = PathBuf::from("untitled");
    }

    let settings = cx.global::<Settings>();
    let language_id = match language_name {
        Some("Plain Text") => "plaintext".to_string(),
        Some(language_name) => language_name.to_lowercase(),
        None => "plaintext".to_string(),
    };
    request::GetCompletionsParams {
        doc: request::GetCompletionsDocument {
            source: buffer.text(),
            tab_size: settings.tab_size(language_name).into(),
            indent_size: 1,
            insert_spaces: !settings.hard_tabs(language_name),
            uri: lsp::Url::from_file_path(&path).unwrap(),
            path: path.to_string_lossy().into(),
            relative_path: relative_path.to_string_lossy().into(),
            language_id,
            position: point_to_lsp(position),
            version: 0,
        },
    }
}

fn completion_from_lsp(completion: request::Completion, buffer: &BufferSnapshot) -> Completion {
    let position = buffer.clip_point_utf16(point_from_lsp(completion.position), Bias::Left);
    Completion {
        position: buffer.anchor_before(position),
        text: completion.display_text,
    }
}

async fn get_copilot_lsp(
    http: Arc<dyn HttpClient>,
    node: Arc<NodeRuntime>,
) -> anyhow::Result<PathBuf> {
    const SERVER_PATH: &'static str = "node_modules/copilot-node-server/copilot/dist/agent.js";

    ///Check for the latest copilot language server and download it if we haven't already
    async fn fetch_latest(
        _http: Arc<dyn HttpClient>,
        node: Arc<NodeRuntime>,
    ) -> anyhow::Result<PathBuf> {
        const COPILOT_NPM_PACKAGE: &'static str = "copilot-node-server";

        let release = node.npm_package_latest_version(COPILOT_NPM_PACKAGE).await?;

        let version_dir = &*paths::COPILOT_DIR.join(format!("copilot-{}", release.clone()));

        fs::create_dir_all(version_dir).await?;
        let server_path = version_dir.join(SERVER_PATH);

        if fs::metadata(&server_path).await.is_err() {
            node.npm_install_packages([(COPILOT_NPM_PACKAGE, release.as_str())], version_dir)
                .await?;

            remove_matching(&paths::COPILOT_DIR, |entry| entry != version_dir).await;
        }

        Ok(server_path)
    }

    match fetch_latest(http, node).await {
        ok @ Result::Ok(..) => ok,
        e @ Err(..) => {
            e.log_err();
            // Fetch a cached binary, if it exists
            (|| async move {
                let mut last_version_dir = None;
                let mut entries = fs::read_dir(paths::COPILOT_DIR.as_path()).await?;
                while let Some(entry) = entries.next().await {
                    let entry = entry?;
                    if entry.file_type().await?.is_dir() {
                        last_version_dir = Some(entry.path());
                    }
                }
                let last_version_dir =
                    last_version_dir.ok_or_else(|| anyhow!("no cached binary"))?;
                let server_path = last_version_dir.join(SERVER_PATH);
                if server_path.exists() {
                    Ok(server_path)
                } else {
                    Err(anyhow!(
                        "missing executable in directory {:?}",
                        last_version_dir
                    ))
                }
            })()
            .await
        }
    }
}
