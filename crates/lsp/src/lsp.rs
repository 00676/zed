use anyhow::{anyhow, Context, Result};
use futures::{io::BufWriter, AsyncRead, AsyncWrite};
use gpui::{executor, Task};
use parking_lot::{Mutex, RwLock};
use postage::{barrier, oneshot, prelude::Stream, sink::Sink, watch};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use smol::{
    channel,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use std::{
    collections::HashMap,
    future::Future,
    io::Write,
    str::FromStr,
    sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Arc,
    },
};
use std::{path::Path, process::Stdio};
use util::TryFutureExt;

pub use lsp_types::*;

const JSON_RPC_VERSION: &'static str = "2.0";
const CONTENT_LEN_HEADER: &'static str = "Content-Length: ";

type NotificationHandler = Box<dyn Send + Sync + FnMut(&str)>;
type ResponseHandler = Box<dyn Send + FnOnce(Result<&str, Error>)>;

pub struct LanguageServer {
    next_id: AtomicUsize,
    outbound_tx: RwLock<Option<channel::Sender<Vec<u8>>>>,
    capabilities: watch::Receiver<Option<ServerCapabilities>>,
    notification_handlers: Arc<RwLock<HashMap<&'static str, NotificationHandler>>>,
    response_handlers: Arc<Mutex<HashMap<usize, ResponseHandler>>>,
    executor: Arc<executor::Background>,
    io_tasks: Mutex<Option<(Task<Option<()>>, Task<Option<()>>)>>,
    initialized: barrier::Receiver,
    output_done_rx: Mutex<Option<barrier::Receiver>>,
}

pub struct Subscription {
    method: &'static str,
    notification_handlers: Arc<RwLock<HashMap<&'static str, NotificationHandler>>>,
}

#[derive(Serialize, Deserialize)]
struct Request<'a, T> {
    jsonrpc: &'a str,
    id: usize,
    method: &'a str,
    params: T,
}

#[derive(Serialize, Deserialize)]
struct AnyResponse<'a> {
    id: usize,
    #[serde(default)]
    error: Option<Error>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
}

#[derive(Serialize, Deserialize)]
struct Notification<'a, T> {
    #[serde(borrow)]
    jsonrpc: &'a str,
    #[serde(borrow)]
    method: &'a str,
    params: T,
}

#[derive(Deserialize)]
struct AnyNotification<'a> {
    #[serde(borrow)]
    method: &'a str,
    #[serde(borrow)]
    params: &'a RawValue,
}

#[derive(Debug, Serialize, Deserialize)]
struct Error {
    message: String,
}

impl LanguageServer {
    pub fn new(
        binary_path: &Path,
        root_path: &Path,
        background: Arc<executor::Background>,
    ) -> Result<Arc<Self>> {
        let mut server = Command::new(binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = server.stdin.take().unwrap();
        let stdout = server.stdout.take().unwrap();
        Self::new_internal(stdin, stdout, root_path, background)
    }

    fn new_internal<Stdin, Stdout>(
        stdin: Stdin,
        stdout: Stdout,
        root_path: &Path,
        executor: Arc<executor::Background>,
    ) -> Result<Arc<Self>>
    where
        Stdin: AsyncWrite + Unpin + Send + 'static,
        Stdout: AsyncRead + Unpin + Send + 'static,
    {
        let mut stdin = BufWriter::new(stdin);
        let mut stdout = BufReader::new(stdout);
        let (outbound_tx, outbound_rx) = channel::unbounded::<Vec<u8>>();
        let notification_handlers = Arc::new(RwLock::new(HashMap::<_, NotificationHandler>::new()));
        let response_handlers = Arc::new(Mutex::new(HashMap::<_, ResponseHandler>::new()));
        let input_task = executor.spawn(
            {
                let notification_handlers = notification_handlers.clone();
                let response_handlers = response_handlers.clone();
                async move {
                    let mut buffer = Vec::new();
                    loop {
                        buffer.clear();
                        stdout.read_until(b'\n', &mut buffer).await?;
                        stdout.read_until(b'\n', &mut buffer).await?;
                        let message_len: usize = std::str::from_utf8(&buffer)?
                            .strip_prefix(CONTENT_LEN_HEADER)
                            .ok_or_else(|| anyhow!("invalid header"))?
                            .trim_end()
                            .parse()?;

                        buffer.resize(message_len, 0);
                        stdout.read_exact(&mut buffer).await?;

                        if let Ok(AnyNotification { method, params }) =
                            serde_json::from_slice(&buffer)
                        {
                            if let Some(handler) = notification_handlers.write().get_mut(method) {
                                handler(params.get());
                            } else {
                                log::info!(
                                    "unhandled notification {}:\n{}",
                                    method,
                                    serde_json::to_string_pretty(
                                        &Value::from_str(params.get()).unwrap()
                                    )
                                    .unwrap()
                                );
                            }
                        } else if let Ok(AnyResponse { id, error, result }) =
                            serde_json::from_slice(&buffer)
                        {
                            if let Some(handler) = response_handlers.lock().remove(&id) {
                                if let Some(error) = error {
                                    handler(Err(error));
                                } else if let Some(result) = result {
                                    handler(Ok(result.get()));
                                } else {
                                    handler(Ok("null"));
                                }
                            }
                        } else {
                            return Err(anyhow!(
                                "failed to deserialize message:\n{}",
                                std::str::from_utf8(&buffer)?
                            ));
                        }
                    }
                }
            }
            .log_err(),
        );
        let (output_done_tx, output_done_rx) = barrier::channel();
        let output_task = executor.spawn(
            async move {
                let mut content_len_buffer = Vec::new();
                while let Ok(message) = outbound_rx.recv().await {
                    content_len_buffer.clear();
                    write!(content_len_buffer, "{}", message.len()).unwrap();
                    stdin.write_all(CONTENT_LEN_HEADER.as_bytes()).await?;
                    stdin.write_all(&content_len_buffer).await?;
                    stdin.write_all("\r\n\r\n".as_bytes()).await?;
                    stdin.write_all(&message).await?;
                    stdin.flush().await?;
                }
                drop(output_done_tx);
                Ok(())
            }
            .log_err(),
        );

        let (initialized_tx, initialized_rx) = barrier::channel();
        let (mut capabilities_tx, capabilities_rx) = watch::channel();
        let this = Arc::new(Self {
            notification_handlers,
            response_handlers,
            capabilities: capabilities_rx,
            next_id: Default::default(),
            outbound_tx: RwLock::new(Some(outbound_tx)),
            executor: executor.clone(),
            io_tasks: Mutex::new(Some((input_task, output_task))),
            initialized: initialized_rx,
            output_done_rx: Mutex::new(Some(output_done_rx)),
        });

        let root_uri = Url::from_file_path(root_path).map_err(|_| anyhow!("invalid root path"))?;
        executor
            .spawn({
                let this = this.clone();
                async move {
                    if let Some(capabilities) = this.init(root_uri).log_err().await {
                        *capabilities_tx.borrow_mut() = Some(capabilities);
                    }

                    drop(initialized_tx);
                }
            })
            .detach();

        Ok(this)
    }

    async fn init(self: Arc<Self>, root_uri: Url) -> Result<ServerCapabilities> {
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Default::default(),
            root_path: Default::default(),
            root_uri: Some(root_uri),
            initialization_options: Default::default(),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    definition: Some(GotoCapability {
                        link_support: Some(true),
                        ..Default::default()
                    }),
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(true),
                            resolve_support: Some(CompletionItemCapabilityResolveSupport {
                                properties: vec!["additionalTextEdits".to_string()],
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                experimental: Some(json!({
                    "serverStatusNotification": true,
                })),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: Default::default(),
            workspace_folders: Default::default(),
            client_info: Default::default(),
            locale: Default::default(),
        };

        let this = self.clone();
        let request = Self::request_internal::<request::Initialize>(
            &this.next_id,
            &this.response_handlers,
            this.outbound_tx.read().as_ref(),
            params,
        );
        let response = request.await?;
        Self::notify_internal::<notification::Initialized>(
            this.outbound_tx.read().as_ref(),
            InitializedParams {},
        )?;
        Ok(response.capabilities)
    }

    pub fn shutdown(&self) -> Option<impl 'static + Send + Future<Output = Result<()>>> {
        if let Some(tasks) = self.io_tasks.lock().take() {
            let response_handlers = self.response_handlers.clone();
            let outbound_tx = self.outbound_tx.write().take();
            let next_id = AtomicUsize::new(self.next_id.load(SeqCst));
            let mut output_done = self.output_done_rx.lock().take().unwrap();
            Some(async move {
                Self::request_internal::<request::Shutdown>(
                    &next_id,
                    &response_handlers,
                    outbound_tx.as_ref(),
                    (),
                )
                .await?;
                Self::notify_internal::<notification::Exit>(outbound_tx.as_ref(), ())?;
                drop(outbound_tx);
                output_done.recv().await;
                drop(tasks);
                Ok(())
            })
        } else {
            None
        }
    }

    pub fn on_notification<T, F>(&self, mut f: F) -> Subscription
    where
        T: notification::Notification,
        F: 'static + Send + Sync + FnMut(T::Params),
    {
        let prev_handler = self.notification_handlers.write().insert(
            T::METHOD,
            Box::new(
                move |notification| match serde_json::from_str(notification) {
                    Ok(notification) => f(notification),
                    Err(err) => log::error!("error parsing notification {}: {}", T::METHOD, err),
                },
            ),
        );

        assert!(
            prev_handler.is_none(),
            "registered multiple handlers for the same notification"
        );

        Subscription {
            method: T::METHOD,
            notification_handlers: self.notification_handlers.clone(),
        }
    }

    pub fn capabilities(&self) -> watch::Receiver<Option<ServerCapabilities>> {
        self.capabilities.clone()
    }

    pub fn request<T: request::Request>(
        self: &Arc<Self>,
        params: T::Params,
    ) -> impl Future<Output = Result<T::Result>>
    where
        T::Result: 'static + Send,
    {
        let this = self.clone();
        async move {
            this.initialized.clone().recv().await;
            Self::request_internal::<T>(
                &this.next_id,
                &this.response_handlers,
                this.outbound_tx.read().as_ref(),
                params,
            )
            .await
        }
    }

    fn request_internal<T: request::Request>(
        next_id: &AtomicUsize,
        response_handlers: &Mutex<HashMap<usize, ResponseHandler>>,
        outbound_tx: Option<&channel::Sender<Vec<u8>>>,
        params: T::Params,
    ) -> impl 'static + Future<Output = Result<T::Result>>
    where
        T::Result: 'static + Send,
    {
        let id = next_id.fetch_add(1, SeqCst);
        let message = serde_json::to_vec(&Request {
            jsonrpc: JSON_RPC_VERSION,
            id,
            method: T::METHOD,
            params,
        })
        .unwrap();
        let mut response_handlers = response_handlers.lock();
        let (mut tx, mut rx) = oneshot::channel();
        response_handlers.insert(
            id,
            Box::new(move |result| {
                let response = match result {
                    Ok(response) => {
                        serde_json::from_str(response).context("failed to deserialize response")
                    }
                    Err(error) => Err(anyhow!("{}", error.message)),
                };
                let _ = tx.try_send(response);
            }),
        );

        let send = outbound_tx
            .as_ref()
            .ok_or_else(|| {
                anyhow!("tried to send a request to a language server that has been shut down")
            })
            .and_then(|outbound_tx| {
                outbound_tx.try_send(message)?;
                Ok(())
            });
        async move {
            send?;
            rx.recv().await.unwrap()
        }
    }

    pub fn notify<T: notification::Notification>(
        self: &Arc<Self>,
        params: T::Params,
    ) -> impl Future<Output = Result<()>> {
        let this = self.clone();
        async move {
            this.initialized.clone().recv().await;
            Self::notify_internal::<T>(this.outbound_tx.read().as_ref(), params)?;
            Ok(())
        }
    }

    fn notify_internal<T: notification::Notification>(
        outbound_tx: Option<&channel::Sender<Vec<u8>>>,
        params: T::Params,
    ) -> Result<()> {
        let message = serde_json::to_vec(&Notification {
            jsonrpc: JSON_RPC_VERSION,
            method: T::METHOD,
            params,
        })
        .unwrap();
        let outbound_tx = outbound_tx
            .as_ref()
            .ok_or_else(|| anyhow!("tried to notify a language server that has been shut down"))?;
        outbound_tx.try_send(message)?;
        Ok(())
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown() {
            self.executor.spawn(shutdown).detach();
        }
    }
}

impl Subscription {
    pub fn detach(mut self) {
        self.method = "";
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.notification_handlers.write().remove(self.method);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct FakeLanguageServer {
    buffer: Vec<u8>,
    stdin: smol::io::BufReader<async_pipe::PipeReader>,
    stdout: smol::io::BufWriter<async_pipe::PipeWriter>,
    pub started: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(any(test, feature = "test-support"))]
pub struct RequestId<T> {
    id: usize,
    _type: std::marker::PhantomData<T>,
}

#[cfg(any(test, feature = "test-support"))]
impl LanguageServer {
    pub async fn fake(executor: Arc<executor::Background>) -> (Arc<Self>, FakeLanguageServer) {
        Self::fake_with_capabilities(Default::default(), executor).await
    }

    pub async fn fake_with_capabilities(
        capabilities: ServerCapabilities,
        executor: Arc<executor::Background>,
    ) -> (Arc<Self>, FakeLanguageServer) {
        let stdin = async_pipe::pipe();
        let stdout = async_pipe::pipe();
        let mut fake = FakeLanguageServer {
            stdin: smol::io::BufReader::new(stdin.1),
            stdout: smol::io::BufWriter::new(stdout.0),
            buffer: Vec::new(),
            started: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        };

        let server = Self::new_internal(stdin.0, stdout.1, Path::new("/"), executor).unwrap();

        let (init_id, _) = fake.receive_request::<request::Initialize>().await;
        fake.respond(
            init_id,
            InitializeResult {
                capabilities,
                ..Default::default()
            },
        )
        .await;
        fake.receive_notification::<notification::Initialized>()
            .await;

        (server, fake)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl FakeLanguageServer {
    pub async fn notify<T: notification::Notification>(&mut self, params: T::Params) {
        if !self.started.load(std::sync::atomic::Ordering::SeqCst) {
            panic!("can't simulate an LSP notification before the server has been started");
        }
        let message = serde_json::to_vec(&Notification {
            jsonrpc: JSON_RPC_VERSION,
            method: T::METHOD,
            params,
        })
        .unwrap();
        self.send(message).await;
    }

    pub async fn respond<'a, T: request::Request>(
        &mut self,
        request_id: RequestId<T>,
        result: T::Result,
    ) {
        let result = serde_json::to_string(&result).unwrap();
        let message = serde_json::to_vec(&AnyResponse {
            id: request_id.id,
            error: None,
            result: Some(&RawValue::from_string(result).unwrap()),
        })
        .unwrap();
        self.send(message).await;
    }

    pub async fn receive_request<T: request::Request>(&mut self) -> (RequestId<T>, T::Params) {
        loop {
            self.receive().await;
            if let Ok(request) = serde_json::from_slice::<Request<T::Params>>(&self.buffer) {
                assert_eq!(request.method, T::METHOD);
                assert_eq!(request.jsonrpc, JSON_RPC_VERSION);
                return (
                    RequestId {
                        id: request.id,
                        _type: std::marker::PhantomData,
                    },
                    request.params,
                );
            } else {
                println!(
                    "skipping message in fake language server {:?}",
                    std::str::from_utf8(&self.buffer)
                );
            }
        }
    }

    pub async fn receive_notification<T: notification::Notification>(&mut self) -> T::Params {
        self.receive().await;
        let notification = serde_json::from_slice::<Notification<T::Params>>(&self.buffer).unwrap();
        assert_eq!(notification.method, T::METHOD);
        notification.params
    }

    pub async fn start_progress(&mut self, token: impl Into<String>) {
        self.notify::<notification::Progress>(ProgressParams {
            token: NumberOrString::String(token.into()),
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(Default::default())),
        })
        .await;
    }

    pub async fn end_progress(&mut self, token: impl Into<String>) {
        self.notify::<notification::Progress>(ProgressParams {
            token: NumberOrString::String(token.into()),
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(Default::default())),
        })
        .await;
    }

    async fn send(&mut self, message: Vec<u8>) {
        self.stdout
            .write_all(CONTENT_LEN_HEADER.as_bytes())
            .await
            .unwrap();
        self.stdout
            .write_all((format!("{}", message.len())).as_bytes())
            .await
            .unwrap();
        self.stdout.write_all("\r\n\r\n".as_bytes()).await.unwrap();
        self.stdout.write_all(&message).await.unwrap();
        self.stdout.flush().await.unwrap();
    }

    async fn receive(&mut self) {
        self.buffer.clear();
        self.stdin
            .read_until(b'\n', &mut self.buffer)
            .await
            .unwrap();
        self.stdin
            .read_until(b'\n', &mut self.buffer)
            .await
            .unwrap();
        let message_len: usize = std::str::from_utf8(&self.buffer)
            .unwrap()
            .strip_prefix(CONTENT_LEN_HEADER)
            .unwrap()
            .trim_end()
            .parse()
            .unwrap();
        self.buffer.resize(message_len, 0);
        self.stdin.read_exact(&mut self.buffer).await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use simplelog::SimpleLogger;
    use unindent::Unindent;
    use util::test::temp_tree;

    #[gpui::test]
    async fn test_rust_analyzer(cx: TestAppContext) {
        let lib_source = r#"
            fn fun() {
                let hello = "world";
            }
        "#
        .unindent();
        let root_dir = temp_tree(json!({
            "Cargo.toml": r#"
                [package]
                name = "temp"
                version = "0.1.0"
                edition = "2018"
            "#.unindent(),
            "src": {
                "lib.rs": &lib_source
            }
        }));
        let lib_file_uri = Url::from_file_path(root_dir.path().join("src/lib.rs")).unwrap();

        let server = cx.read(|cx| {
            LanguageServer::new(
                Path::new("rust-analyzer"),
                root_dir.path(),
                cx.background().clone(),
            )
            .unwrap()
        });
        server.next_idle_notification().await;

        server
            .notify::<notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    lib_file_uri.clone(),
                    "rust".to_string(),
                    0,
                    lib_source,
                ),
            })
            .await
            .unwrap();

        let hover = server
            .request::<request::HoverRequest>(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier::new(lib_file_uri),
                    position: Position::new(1, 21),
                },
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            hover.contents,
            HoverContents::Markup(MarkupContent {
                kind: MarkupKind::PlainText,
                value: "&str".to_string()
            })
        );
    }

    #[gpui::test]
    async fn test_fake(cx: TestAppContext) {
        SimpleLogger::init(log::LevelFilter::Info, Default::default()).unwrap();

        let (server, mut fake) = LanguageServer::fake(cx.background()).await;

        let (message_tx, message_rx) = channel::unbounded();
        let (diagnostics_tx, diagnostics_rx) = channel::unbounded();
        server
            .on_notification::<notification::ShowMessage, _>(move |params| {
                message_tx.try_send(params).unwrap()
            })
            .detach();
        server
            .on_notification::<notification::PublishDiagnostics, _>(move |params| {
                diagnostics_tx.try_send(params).unwrap()
            })
            .detach();

        server
            .notify::<notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    Url::from_str("file://a/b").unwrap(),
                    "rust".to_string(),
                    0,
                    "".to_string(),
                ),
            })
            .await
            .unwrap();
        assert_eq!(
            fake.receive_notification::<notification::DidOpenTextDocument>()
                .await
                .text_document
                .uri
                .as_str(),
            "file://a/b"
        );

        fake.notify::<notification::ShowMessage>(ShowMessageParams {
            typ: MessageType::ERROR,
            message: "ok".to_string(),
        })
        .await;
        fake.notify::<notification::PublishDiagnostics>(PublishDiagnosticsParams {
            uri: Url::from_str("file://b/c").unwrap(),
            version: Some(5),
            diagnostics: vec![],
        })
        .await;
        assert_eq!(message_rx.recv().await.unwrap().message, "ok");
        assert_eq!(
            diagnostics_rx.recv().await.unwrap().uri.as_str(),
            "file://b/c"
        );

        drop(server);
        let (shutdown_request, _) = fake.receive_request::<request::Shutdown>().await;
        fake.respond(shutdown_request, ()).await;
        fake.receive_notification::<notification::Exit>().await;
    }

    impl LanguageServer {
        async fn next_idle_notification(self: &Arc<Self>) {
            let (tx, rx) = channel::unbounded();
            let _subscription =
                self.on_notification::<ServerStatusNotification, _>(move |params| {
                    if params.quiescent {
                        tx.try_send(()).unwrap();
                    }
                });
            let _ = rx.recv().await;
        }
    }

    pub enum ServerStatusNotification {}

    impl notification::Notification for ServerStatusNotification {
        type Params = ServerStatusParams;
        const METHOD: &'static str = "experimental/serverStatus";
    }

    #[derive(Deserialize, Serialize, PartialEq, Eq, Clone)]
    pub struct ServerStatusParams {
        pub quiescent: bool,
    }
}
