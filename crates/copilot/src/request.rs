use serde::{Deserialize, Serialize};

pub enum CheckStatus {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckStatusParams {
    pub local_checks_only: bool,
}

impl lsp::request::Request for CheckStatus {
    type Params = CheckStatusParams;
    type Result = SignInStatus;
    const METHOD: &'static str = "checkStatus";
}

pub enum SignInInitiate {}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignInInitiateParams {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum SignInInitiateResult {
    AlreadySignedIn { user: String },
    PromptUserDeviceFlow(PromptUserDeviceFlow),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptUserDeviceFlow {
    pub user_code: String,
    pub verification_uri: String,
}

impl lsp::request::Request for SignInInitiate {
    type Params = SignInInitiateParams;
    type Result = SignInInitiateResult;
    const METHOD: &'static str = "signInInitiate";
}

pub enum SignInConfirm {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInConfirmParams {
    pub user_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum SignInStatus {
    #[serde(rename = "OK")]
    Ok {
        user: String,
    },
    MaybeOk {
        user: String,
    },
    AlreadySignedIn {
        user: String,
    },
    NotAuthorized {
        user: String,
    },
    NotSignedIn,
}

impl lsp::request::Request for SignInConfirm {
    type Params = SignInConfirmParams;
    type Result = SignInStatus;
    const METHOD: &'static str = "signInConfirm";
}

pub enum SignOut {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignOutParams {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignOutResult {}

impl lsp::request::Request for SignOut {
    type Params = SignOutParams;
    type Result = SignOutResult;
    const METHOD: &'static str = "signOut";
}

pub enum GetCompletions {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCompletionsParams {
    pub doc: GetCompletionsDocument,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCompletionsDocument {
    pub source: String,
    pub tab_size: u32,
    pub indent_size: u32,
    pub insert_spaces: bool,
    pub uri: lsp::Url,
    pub path: String,
    pub relative_path: String,
    pub language_id: String,
    pub position: lsp::Position,
    pub version: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCompletionsResult {
    pub completions: Vec<Completion>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    pub text: String,
    pub position: lsp::Position,
    pub uuid: String,
    pub range: lsp::Range,
    pub display_text: String,
}

impl lsp::request::Request for GetCompletions {
    type Params = GetCompletionsParams;
    type Result = GetCompletionsResult;
    const METHOD: &'static str = "getCompletions";
}

pub enum GetCompletionsCycling {}

impl lsp::request::Request for GetCompletionsCycling {
    type Params = GetCompletionsParams;
    type Result = GetCompletionsResult;
    const METHOD: &'static str = "getCompletionsCycling";
}
