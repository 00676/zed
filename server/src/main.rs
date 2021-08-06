mod admin;
mod assets;
mod auth;
mod db;
mod env;
mod errors;
mod expiring;
mod github;
mod home;
mod rpc;
mod team;
#[cfg(test)]
mod tests;

use self::errors::TideResultExt as _;
use anyhow::{Context, Result};
use async_std::{net::TcpListener, sync::RwLock as AsyncRwLock};
use async_trait::async_trait;
use auth::RequestExt as _;
use db::{Db, DbOptions};
use handlebars::{Handlebars, TemplateRenderError};
use parking_lot::RwLock;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use surf::http::cookies::SameSite;
use tide::{log, sessions::SessionMiddleware};
use tide_compress::CompressMiddleware;
use zrpc::Peer;

type Request = tide::Request<Arc<AppState>>;

#[derive(RustEmbed)]
#[folder = "templates"]
struct Templates;

#[derive(Default, Deserialize)]
pub struct Config {
    pub http_port: u16,
    pub database_url: String,
    pub session_secret: String,
    pub github_app_id: usize,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub github_private_key: String,
}

pub struct AppState {
    db: Db,
    handlebars: RwLock<Handlebars<'static>>,
    auth_client: auth::Client,
    github_client: Arc<github::AppClient>,
    repo_client: github::RepoClient,
    rpc: AsyncRwLock<rpc::State>,
    config: Config,
}

impl AppState {
    async fn new(config: Config) -> tide::Result<Arc<Self>> {
        let db = Db(DbOptions::new()
            .max_connections(5)
            .connect(&config.database_url)
            .await
            .context("failed to connect to postgres database")?);

        let github_client =
            github::AppClient::new(config.github_app_id, config.github_private_key.clone());
        let repo_client = github_client
            .repo("zed-industries/zed".into())
            .await
            .context("failed to initialize github client")?;

        let this = Self {
            db,
            handlebars: Default::default(),
            auth_client: auth::build_client(&config.github_client_id, &config.github_client_secret),
            github_client,
            repo_client,
            rpc: Default::default(),
            config,
        };
        this.register_partials();
        Ok(Arc::new(this))
    }

    fn register_partials(&self) {
        for path in Templates::iter() {
            if let Some(partial_name) = path
                .strip_prefix("partials/")
                .and_then(|path| path.strip_suffix(".hbs"))
            {
                let partial = Templates::get(path.as_ref()).unwrap();
                self.handlebars
                    .write()
                    .register_partial(partial_name, std::str::from_utf8(partial.as_ref()).unwrap())
                    .unwrap()
            }
        }
    }

    fn render_template(
        &self,
        path: &'static str,
        data: &impl Serialize,
    ) -> Result<String, TemplateRenderError> {
        #[cfg(debug_assertions)]
        self.register_partials();

        self.handlebars.read().render_template(
            std::str::from_utf8(Templates::get(path).unwrap().as_ref()).unwrap(),
            data,
        )
    }
}

#[async_trait]
trait RequestExt {
    async fn layout_data(&mut self) -> tide::Result<Arc<LayoutData>>;
    fn db(&self) -> &Db;
}

#[async_trait]
impl RequestExt for Request {
    async fn layout_data(&mut self) -> tide::Result<Arc<LayoutData>> {
        if self.ext::<Arc<LayoutData>>().is_none() {
            self.set_ext(Arc::new(LayoutData {
                current_user: self.current_user().await?,
            }));
        }
        Ok(self.ext::<Arc<LayoutData>>().unwrap().clone())
    }

    fn db(&self) -> &Db {
        &self.state().db
    }
}

#[derive(Serialize)]
struct LayoutData {
    current_user: Option<auth::User>,
}

#[async_std::main]
async fn main() -> tide::Result<()> {
    log::start();

    if let Err(error) = env::load_dotenv() {
        log::error!(
            "error loading .env.toml (this is expected in production): {}",
            error
        );
    }

    let config = envy::from_env::<Config>().expect("error loading config");
    let state = AppState::new(config).await?;
    let rpc = Peer::new();
    run_server(
        state.clone(),
        rpc,
        TcpListener::bind(&format!("0.0.0.0:{}", state.config.http_port)).await?,
    )
    .await?;
    Ok(())
}

pub async fn run_server(
    state: Arc<AppState>,
    rpc: Arc<Peer>,
    listener: TcpListener,
) -> tide::Result<()> {
    let mut web = tide::with_state(state.clone());
    web.with(CompressMiddleware::new());
    web.with(
        SessionMiddleware::new(
            db::SessionStore::new_with_table_name(&state.config.database_url, "sessions")
                .await
                .unwrap(),
            state.config.session_secret.as_bytes(),
        )
        .with_same_site_policy(SameSite::Lax), // Required obtain our session in /auth_callback
    );
    web.with(errors::Middleware);
    home::add_routes(&mut web);
    team::add_routes(&mut web);
    admin::add_routes(&mut web);
    auth::add_routes(&mut web);
    assets::add_routes(&mut web);

    let mut app = tide::with_state(state.clone());
    rpc::add_routes(&mut app, &rpc);
    app.at("/").nest(web);

    app.listen(listener).await?;

    Ok(())
}
