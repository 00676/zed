use anyhow::{anyhow, Result};
use async_trait::async_trait;
use client::http::HttpClient;
use futures::lock::Mutex;
use gpui::executor::Background;
use language::{LanguageServerName, LspAdapterTrait};
use plugin_runtime::{Plugin, PluginBuilder, WasiFn};
use std::{any::Any, path::PathBuf, sync::Arc};
use util::ResultExt;

pub async fn new_json(executor: Arc<Background>) -> Result<PluginLspAdapter> {
    let plugin = PluginBuilder::new_with_default_ctx()?
        .host_function_async("command", |command: String| async move {
            let mut args = command.split(' ');
            let command = args.next().unwrap();
            smol::process::Command::new(command)
                .args(args)
                .output()
                .await
                .log_err()
                .map(|output| output.stdout)
        })?
        .init(
            true,
            include_bytes!("../../../../plugins/bin/json_language.wasm.pre"),
        )
        .await?;
    PluginLspAdapter::new(plugin, executor).await
}

pub struct PluginLspAdapter {
    name: WasiFn<(), String>,
    server_args: WasiFn<(), Vec<String>>,
    fetch_latest_server_version: WasiFn<(), Option<String>>,
    fetch_server_binary: WasiFn<(PathBuf, String), Result<PathBuf, String>>,
    cached_server_binary: WasiFn<PathBuf, Option<PathBuf>>,
    // label_for_completion: WasiFn<String, Option<String>>,
    initialization_options: WasiFn<(), String>,
    executor: Arc<Background>,
    runtime: Arc<Mutex<Plugin>>,
}

impl PluginLspAdapter {
    pub async fn new(mut plugin: Plugin, executor: Arc<Background>) -> Result<Self> {
        Ok(Self {
            name: plugin.function("name")?,
            server_args: plugin.function("server_args")?,
            fetch_latest_server_version: plugin.function("fetch_latest_server_version")?,
            fetch_server_binary: plugin.function("fetch_server_binary")?,
            cached_server_binary: plugin.function("cached_server_binary")?,
            // label_for_completion: plugin.function("label_for_completion")?,
            initialization_options: plugin.function("initialization_options")?,
            executor,
            runtime: Arc::new(Mutex::new(plugin)),
        })
    }
}

#[async_trait]
impl LspAdapterTrait for PluginLspAdapter {
    async fn name(&self) -> LanguageServerName {
        let name: String = self
            .runtime
            .lock()
            .await
            .call(&self.name, ())
            .await
            .unwrap();
        LanguageServerName(name.into())
    }

    async fn server_args<'a>(&'a self) -> Vec<String> {
        self.runtime
            .lock()
            .await
            .call(&self.server_args, ())
            .await
            .unwrap()
    }

    async fn fetch_latest_server_version(
        &self,
        _: Arc<dyn HttpClient>,
    ) -> Result<Box<dyn 'static + Send + Any>> {
        let runtime = self.runtime.clone();
        let function = self.fetch_latest_server_version;
        self.executor
            .spawn(async move {
                let mut runtime = runtime.lock().await;
                let versions: Result<Option<String>> =
                    runtime.call::<_, Option<String>>(&function, ()).await;
                versions
                    .map_err(|e| anyhow!("{}", e))?
                    .ok_or_else(|| anyhow!("Could not fetch latest server version"))
                    .map(|v| Box::new(v) as Box<_>)
            })
            .await
    }

    async fn fetch_server_binary(
        &self,
        version: Box<dyn 'static + Send + Any>,
        _: Arc<dyn HttpClient>,
        container_dir: PathBuf,
    ) -> Result<PathBuf> {
        let version = *version.downcast::<String>().unwrap();
        let runtime = self.runtime.clone();
        let function = self.fetch_server_binary;
        self.executor
            .spawn(async move {
                let mut runtime = runtime.lock().await;
                let handle = runtime.attach_path(&container_dir)?;
                let result: Result<PathBuf, String> =
                    runtime.call(&function, (container_dir, version)).await?;
                runtime.remove_resource(handle)?;
                result.map_err(|e| anyhow!("{}", e))
            })
            .await
    }

    async fn cached_server_binary(&self, container_dir: PathBuf) -> Option<PathBuf> {
        let runtime = self.runtime.clone();
        let function = self.cached_server_binary;

        self.executor
            .spawn(async move {
                let mut runtime = runtime.lock().await;
                let handle = runtime.attach_path(&container_dir).ok()?;
                let result: Option<PathBuf> = runtime.call(&function, container_dir).await.ok()?;
                runtime.remove_resource(handle).ok()?;
                result
            })
            .await
    }

    // async fn process_diagnostics(&self, _: &mut lsp::PublishDiagnosticsParams) {}

    // fn label_for_completion(
    //     &self,
    //     item: &lsp::CompletionItem,
    //     language: &language::Language,
    // ) -> Option<language::CodeLabel> {
    //     // TODO: Push more of this method down into the plugin.
    //     use lsp::CompletionItemKind as Kind;
    //     let len = item.label.len();
    //     let grammar = language.grammar()?;
    //     let kind = format!("{:?}", item.kind?);
    //     let name: String = call_block!(self, &self.label_for_completion, kind).log_err()??;
    //     let highlight_id = grammar.highlight_id_for_name(&name)?;
    //     Some(language::CodeLabel {
    //         text: item.label.clone(),
    //         runs: vec![(0..len, highlight_id)],
    //         filter_range: 0..len,
    //     })
    // }

    async fn initialization_options(&self) -> Option<serde_json::Value> {
        let string: String = self
            .runtime
            .lock()
            .await
            .call(&self.initialization_options, ())
            .await
            .log_err()?;

        serde_json::from_str(&string).ok()
    }
}
