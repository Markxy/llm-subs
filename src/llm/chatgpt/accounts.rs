//! ChatGPT accounts connected with Sign in with ChatGPT, one per provider entry in
//! the configuration, plus the live sign-in and model-list state the window shows.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::oauth::{self, AuthorizeOptions, CallbackOutcome, LoopbackListener, TokenError, TokenGrant};
use super::protect::{protect, unprotect, write_private_file};
use crate::llm::types::LlmError;

const FILE_VERSION: u32 = 1;
/// Refresh a little before the access token runs out rather than mid-request.
const REFRESH_MARGIN_SECONDS: i64 = 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct AuthFile {
    version: u32,
    /// Identifies this installation to the authorization server; generated once.
    host_id: String,
    accounts: BTreeMap<String, StoredAccount>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredAccount {
    /// Issued at the first sign-in and reused for every later token request.
    client_id: String,
    subject: Option<String>,
    email: Option<String>,
    name: Option<String>,
    scopes: Vec<String>,
    tokens: Option<StoredTokens>,
    plan_notice_acknowledged: bool,
}

impl StoredAccount {
    fn usage_sharing(&self) -> bool {
        self.scopes.iter().any(|scope| scope == oauth::USAGE_SHARING_SCOPE)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
    earliest_refresh_at: Option<DateTime<Utc>>,
}

impl StoredTokens {
    fn from_grant(grant: &TokenGrant, previous_refresh_token: Option<String>) -> Self {
        Self {
            access_token: grant.access_token.clone(),
            refresh_token: grant.refresh_token.clone().or(previous_refresh_token),
            expires_at: grant.expires_at,
            earliest_refresh_at: grant.earliest_refresh_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSummary {
    pub email: Option<String>,
    pub name: Option<String>,
    /// The user allowed LLM Subs to use their plan. Without it no request is possible.
    pub usage_sharing: bool,
    pub plan_notice_acknowledged: bool,
}

impl AccountSummary {
    pub fn display_name(&self) -> String {
        match (&self.name, &self.email) {
            (Some(name), Some(email)) if name != email => format!("{name} ({email})"),
            (_, Some(email)) => email.clone(),
            (Some(name), None) => name.clone(),
            (None, None) => "ChatGPT account".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SignInProgress {
    #[default]
    Idle,
    /// The browser was sent to this page and LLM Subs waits for the redirect back.
    Waiting { authorize_url: String },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptModel {
    /// Sent as `model`.
    pub slug: String,
    pub display_name: String,
    /// Effort levels the model accepts, when the catalog says so.
    pub reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ModelCatalog {
    #[default]
    NotLoaded,
    Loading,
    Loaded(Vec<ChatGptModel>),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct AccountView {
    pub account: Option<AccountSummary>,
    /// A sign-in was completed once, so the next one reuses the registration.
    pub registered: bool,
    pub sign_in: SignInProgress,
    pub models: ModelCatalog,
}

#[derive(Default)]
struct LiveState {
    sign_in: SignInProgress,
    /// Counts sign-in attempts so a superseded attempt does not overwrite the newer one's state.
    sign_in_attempt: u64,
    cancel: Option<CancellationToken>,
    models: ModelCatalog,
}

struct Inner {
    path: PathBuf,
    file: Mutex<AuthFile>,
    /// Serialises token refreshes: refresh tokens rotate, so two concurrent refreshes
    /// would invalidate each other.
    token_lock: tokio::sync::Mutex<()>,
    http: reqwest::Client,
    live: Mutex<HashMap<String, LiveState>>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Clone)]
pub struct ChatGptAccounts {
    inner: Arc<Inner>,
}

impl ChatGptAccounts {
    pub fn open(path: &Path) -> Self {
        let mut file = read_auth_file(path).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the ChatGPT sign-in file; starting signed out");
            AuthFile::default()
        });
        let needs_host_id = file.host_id.is_empty();
        if needs_host_id {
            file.host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        }
        file.version = FILE_VERSION;
        crate::llm::install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client");
        let accounts = Self {
            inner: Arc::new(Inner {
                path: path.to_path_buf(),
                file: Mutex::new(file),
                token_lock: tokio::sync::Mutex::new(()),
                http,
                live: Mutex::new(HashMap::new()),
                on_change: Mutex::new(None),
            }),
        };
        if needs_host_id && let Err(error) = accounts.save() {
            tracing::warn!(%error, "could not save the ChatGPT sign-in file");
        }
        accounts
    }

    pub fn set_change_listener(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.inner.on_change.lock() = Some(callback);
    }

    pub fn view(&self, provider: &str) -> AccountView {
        let (account, registered) = {
            let file = self.inner.file.lock();
            let stored = file.accounts.get(provider);
            (stored.and_then(summarize), stored.is_some_and(|a| oauth::is_valid_issued_client_id(&a.client_id)))
        };
        let live = self.inner.live.lock();
        let state = live.get(provider);
        AccountView {
            account,
            registered,
            sign_in: state.map(|s| s.sign_in.clone()).unwrap_or_default(),
            models: state.map(|s| s.models.clone()).unwrap_or_default(),
        }
    }

    pub fn account(&self, provider: &str) -> Option<AccountSummary> {
        self.inner.file.lock().accounts.get(provider).and_then(summarize)
    }

    /// Runs the whole browser sign-in. `on_authorize_url` is called with the page the
    /// user has to open; the caller decides whether to launch a browser or print it.
    pub async fn sign_in(&self, provider: &str, reconsent: bool, on_authorize_url: impl Fn(&str) + Send + Sync) -> Result<AccountSummary> {
        let cancel = CancellationToken::new();
        let mut attempt = 0;
        self.update_live(provider, |live| {
            if let Some(previous) = live.cancel.replace(cancel.clone()) {
                previous.cancel();
            }
            live.sign_in_attempt += 1;
            attempt = live.sign_in_attempt;
            live.sign_in = SignInProgress::Waiting { authorize_url: String::new() };
        });
        let result = tokio::select! {
            result = self.run_sign_in(provider, reconsent, &on_authorize_url) => result,
            _ = cancel.cancelled() => Err(anyhow!("sign-in cancelled")),
            _ = tokio::time::sleep(oauth::SIGN_IN_TIMEOUT) => Err(anyhow!("sign-in timed out; start it again when you are ready")),
        };
        self.update_live(provider, |live| {
            if live.sign_in_attempt != attempt {
                return;
            }
            live.cancel = None;
            live.sign_in = match &result {
                Err(error) if !cancel.is_cancelled() => SignInProgress::Failed(format!("{error:#}")),
                _ => SignInProgress::Idle,
            };
            if result.is_ok() {
                live.models = ModelCatalog::NotLoaded;
            }
        });
        result
    }

    pub fn cancel_sign_in(&self, provider: &str) {
        if let Some(cancel) = self.inner.live.lock().get_mut(provider).and_then(|live| live.cancel.take()) {
            cancel.cancel();
        }
        self.update_live(provider, |live| live.sign_in = SignInProgress::Idle);
    }

    async fn run_sign_in(&self, provider: &str, reconsent: bool, on_authorize_url: &(impl Fn(&str) + Send + Sync)) -> Result<AccountSummary> {
        let listener = LoopbackListener::bind().await?;
        let (previous, host_id) = {
            let file = self.inner.file.lock();
            (file.accounts.get(provider).cloned(), file.host_id.clone())
        };
        let saved_client_id = previous.as_ref().map(|a| a.client_id.clone()).filter(|id| oauth::is_valid_issued_client_id(id));
        let login_hint = previous.as_ref().and_then(|a| a.email.clone());
        let request = oauth::authorization_request(
            &listener.redirect_uri(),
            &AuthorizeOptions { client_id: saved_client_id.as_deref(), host_id: &host_id, login_hint: login_hint.as_deref(), reconsent },
        );
        self.update_live(provider, |live| live.sign_in = SignInProgress::Waiting { authorize_url: request.url.clone() });
        on_authorize_url(&request.url);

        let (code, returned_client_id) = match listener.wait_for_callback(&request.state).await? {
            CallbackOutcome::Approved { code, client_id } => (code, client_id),
            CallbackOutcome::Declined(reason) => bail!("the connection was not approved ({reason})"),
        };
        let client_id = match (saved_client_id, returned_client_id) {
            (Some(saved), Some(returned)) if saved != returned => bail!("ChatGPT answered for a different app registration; try again"),
            (Some(saved), _) => saved,
            (None, Some(returned)) if oauth::is_valid_issued_client_id(&returned) => returned,
            (None, _) => bail!("ChatGPT did not register LLM Subs; try again"),
        };
        // Kept even if the exchange below fails, so the next attempt reuses this registration.
        self.modify(|file| file.accounts.entry(provider.to_string()).or_default().client_id = client_id.clone())?;

        let grant = oauth::exchange_code(&self.inner.http, &client_id, &code, &request.verifier, &request.redirect_uri).await?;
        let id_token = grant.id_token.as_deref().context("ChatGPT did not return an identity for the account")?;
        let claims = oauth::read_id_token(id_token, &client_id, Some(&request.nonce))?;
        if let Some(subject) = previous.as_ref().and_then(|a| a.subject.as_deref())
            && subject != claims.sub
        {
            bail!("this is a different ChatGPT account than the one connected before; choose \"Use a different account\" to switch");
        }
        let scopes = grant.scopes.clone().unwrap_or_default();
        self.modify(|file| {
            let account = file.accounts.entry(provider.to_string()).or_default();
            account.subject = Some(claims.sub.clone());
            account.email = claims.email.clone();
            account.name = claims.name.clone();
            account.scopes = scopes;
            account.tokens = Some(StoredTokens::from_grant(&grant, None));
        })?;
        self.account(provider).context("the account vanished after signing in")
    }

    /// Revokes the session and forgets the tokens. The app registration and the
    /// account's identity stay, so signing in again is quick.
    pub async fn sign_out(&self, provider: &str) -> Result<()> {
        let _guard = self.inner.token_lock.lock().await;
        let account = self.inner.file.lock().accounts.get(provider).cloned();
        let Some(account) = account else { return Ok(()) };
        if let Some(refresh_token) = account.tokens.as_ref().and_then(|t| t.refresh_token.clone())
            && let Err(error) = oauth::revoke(&self.inner.http, &account.client_id, &refresh_token).await
        {
            tracing::warn!(%error, "could not revoke the ChatGPT session; forgetting it locally");
        }
        self.modify(|file| {
            if let Some(account) = file.accounts.get_mut(provider) {
                account.tokens = None;
            }
        })?;
        self.update_live(provider, |live| live.models = ModelCatalog::NotLoaded);
        Ok(())
    }

    /// Drops the registration too, so the next sign-in can use another ChatGPT account.
    pub async fn forget(&self, provider: &str) -> Result<()> {
        self.sign_out(provider).await?;
        self.modify(|file| {
            file.accounts.remove(provider);
        })
    }

    pub fn acknowledge_plan_notice(&self, provider: &str) {
        let result = self.modify(|file| {
            if let Some(account) = file.accounts.get_mut(provider) {
                account.plan_notice_acknowledged = true;
            }
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not save the ChatGPT sign-in file");
        }
    }

    /// Moves an account to a renamed provider entry.
    pub fn rename(&self, from: &str, to: &str) {
        let result = self.modify(|file| {
            if !file.accounts.contains_key(to)
                && let Some(account) = file.accounts.remove(from)
            {
                file.accounts.insert(to.to_string(), account);
            }
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not save the ChatGPT sign-in file");
        }
        let mut live = self.inner.live.lock();
        if let Some(state) = live.remove(from) {
            live.insert(to.to_string(), state);
        }
    }

    /// A valid access token, refreshed first when it is about to expire.
    pub async fn access_token(&self, provider: &str) -> Result<String, LlmError> {
        let _guard = self.inner.token_lock.lock().await;
        self.reload();
        let (account, tokens) = self.usable_account(provider)?;
        let now = Utc::now();
        if tokens.expires_at > now + chrono::Duration::seconds(REFRESH_MARGIN_SECONDS) {
            return Ok(tokens.access_token);
        }
        if tokens.earliest_refresh_at.is_some_and(|earliest| earliest > now) {
            if tokens.expires_at > now {
                return Ok(tokens.access_token);
            }
            return Err(LlmError::Http { status: 503, body: "the ChatGPT session cannot be refreshed yet".into() });
        }
        self.refresh_locked(provider, &account, &tokens).await
    }

    /// Called after the API rejected `rejected_token`: refreshes unless another
    /// request already did.
    pub async fn refresh_rejected(&self, provider: &str, rejected_token: &str) -> Result<String, LlmError> {
        let _guard = self.inner.token_lock.lock().await;
        self.reload();
        let (account, tokens) = self.usable_account(provider)?;
        if tokens.access_token != rejected_token {
            return Ok(tokens.access_token);
        }
        if tokens.earliest_refresh_at.is_some_and(|earliest| earliest > Utc::now()) {
            return Err(LlmError::Unauthorized { status: 401, body: "ChatGPT did not accept the session".into() });
        }
        self.refresh_locked(provider, &account, &tokens).await
    }

    fn usable_account(&self, provider: &str) -> Result<(StoredAccount, StoredTokens), LlmError> {
        let not_signed_in = || LlmError::SignInRequired(format!("provider '{provider}' is not signed in to ChatGPT; sign in under Models"));
        let account = self.inner.file.lock().accounts.get(provider).cloned().ok_or_else(not_signed_in)?;
        let tokens = account.tokens.clone().ok_or_else(not_signed_in)?;
        if !account.usage_sharing() {
            return Err(LlmError::SignInRequired(
                "LLM Subs is not allowed to use this ChatGPT plan; choose \"Enable usage sharing\" under Models".into(),
            ));
        }
        Ok((account, tokens))
    }

    async fn refresh_locked(&self, provider: &str, account: &StoredAccount, tokens: &StoredTokens) -> Result<String, LlmError> {
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            self.clear_tokens(provider);
            return Err(LlmError::SignInRequired("the ChatGPT session has expired; sign in again under Models".into()));
        };
        match oauth::refresh(&self.inner.http, &account.client_id, &refresh_token).await {
            Ok(grant) => {
                // Persist the rotated refresh token before anything else can go wrong.
                let new_tokens = StoredTokens::from_grant(&grant, Some(refresh_token));
                self.modify(|file| {
                    if let Some(stored) = file.accounts.get_mut(provider) {
                        stored.tokens = Some(new_tokens.clone());
                        if let Some(scopes) = &grant.scopes {
                            stored.scopes = scopes.clone();
                        }
                    }
                })
                .map_err(|e| LlmError::Network(format!("saving the refreshed ChatGPT session: {e:#}")))?;
                if let Some(id_token) = grant.id_token.as_deref() {
                    match oauth::read_id_token(id_token, &account.client_id, None) {
                        Ok(claims) if account.subject.as_deref().is_some_and(|subject| subject != claims.sub) => {
                            self.clear_tokens(provider);
                            return Err(LlmError::SignInRequired("ChatGPT switched to a different account; sign in again".into()));
                        }
                        Ok(claims) => {
                            let _ = self.modify(|file| {
                                if let Some(stored) = file.accounts.get_mut(provider) {
                                    stored.email = claims.email.or(stored.email.take());
                                    stored.name = claims.name.or(stored.name.take());
                                }
                            });
                        }
                        Err(error) => tracing::warn!(%error, "ignoring an unreadable refreshed ID token"),
                    }
                }
                Ok(grant.access_token)
            }
            Err(TokenError::SignInAgain(message)) => {
                self.clear_tokens(provider);
                Err(LlmError::SignInRequired(format!("the ChatGPT session has ended ({message}); sign in again under Models")))
            }
            Err(TokenError::Unavailable(message)) => Err(LlmError::Network(message)),
            Err(error @ TokenError::Rejected(_)) => Err(LlmError::Unauthorized { status: 400, body: error.to_string() }),
        }
    }

    fn clear_tokens(&self, provider: &str) {
        let result = self.modify(|file| {
            if let Some(account) = file.accounts.get_mut(provider) {
                account.tokens = None;
            }
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not save the ChatGPT sign-in file");
        }
    }

    /// Fetches the models this account may use and keeps them for the model picker.
    pub async fn load_models(&self, provider: &str) -> Result<Vec<ChatGptModel>> {
        self.update_live(provider, |live| live.models = ModelCatalog::Loading);
        let result = self.fetch_models(provider).await;
        self.update_live(provider, |live| {
            live.models = match &result {
                Ok(models) => ModelCatalog::Loaded(models.clone()),
                Err(error) => ModelCatalog::Failed(format!("{error:#}")),
            }
        });
        result
    }

    async fn fetch_models(&self, provider: &str) -> Result<Vec<ChatGptModel>> {
        let url = format!("{}/models", super::BASE_URL);
        let mut token = self.access_token(provider).await?;
        for attempt in 0..2 {
            let response = self.inner.http.get(&url).bearer_auth(&token).header("accept", "application/json").send().await?;
            let status = response.status();
            let body = response.text().await?;
            if status.as_u16() == 401 && attempt == 0 {
                token = self.refresh_rejected(provider, &token).await?;
                continue;
            }
            if !status.is_success() {
                return Err(super::response_error(status.as_u16(), &body).into());
            }
            return parse_model_catalog(&body);
        }
        bail!("ChatGPT did not accept the session")
    }

    fn update_live(&self, provider: &str, change: impl FnOnce(&mut LiveState)) {
        change(self.inner.live.lock().entry(provider.to_string()).or_default());
        self.notify();
    }

    fn notify(&self) {
        let callback = self.inner.on_change.lock().clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn modify(&self, change: impl FnOnce(&mut AuthFile)) -> Result<()> {
        let snapshot = {
            let mut file = self.inner.file.lock();
            change(&mut file);
            file.clone()
        };
        let result = write_auth_file(&self.inner.path, &snapshot);
        self.notify();
        result
    }

    fn save(&self) -> Result<()> {
        let snapshot = self.inner.file.lock().clone();
        write_auth_file(&self.inner.path, &snapshot)
    }

    /// Picks up tokens another LLM Subs process (the CLI next to the window, say)
    /// refreshed in the meantime.
    fn reload(&self) {
        if let Ok(file) = read_auth_file(&self.inner.path)
            && !file.host_id.is_empty()
        {
            *self.inner.file.lock() = file;
        }
    }
}

fn summarize(account: &StoredAccount) -> Option<AccountSummary> {
    account.tokens.as_ref()?;
    Some(AccountSummary {
        email: account.email.clone(),
        name: account.name.clone(),
        usage_sharing: account.usage_sharing(),
        plan_notice_acknowledged: account.plan_notice_acknowledged,
    })
}

fn read_auth_file(path: &Path) -> Result<AuthFile> {
    let sealed = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(AuthFile::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let plain = unprotect(&sealed)?;
    serde_json::from_slice(&plain).with_context(|| format!("parsing {}", path.display()))
}

fn write_auth_file(path: &Path, file: &AuthFile) -> Result<()> {
    let plain = serde_json::to_vec_pretty(file)?;
    write_private_file(path, &protect(&plain)?)
}

/// Reads `{"models": [{"slug", "display_name", "visibility", ...}]}`, keeping the
/// server's order and only the models meant to be listed.
pub fn parse_model_catalog(body: &str) -> Result<Vec<ChatGptModel>> {
    let value: Value = serde_json::from_str(body).context("unreadable model list")?;
    let entries = value["models"].as_array().or_else(|| value["data"].as_array()).context("the model list is empty")?;
    let models = entries
        .iter()
        .filter(|entry| entry["visibility"].as_str().is_none_or(|visibility| visibility == "list"))
        .filter_map(|entry| {
            let slug = entry["slug"].as_str().or_else(|| entry["id"].as_str())?.to_string();
            let reasoning_levels = entry["supported_reasoning_levels"]
                .as_array()
                .map(|levels| {
                    levels.iter().filter_map(|level| level.as_str().or_else(|| level["effort"].as_str())).map(str::to_string).collect()
                })
                .unwrap_or_default();
            Some(ChatGptModel {
                display_name: entry["display_name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&slug).to_string(),
                slug,
                reasoning_levels,
                default_reasoning_level: entry["default_reasoning_level"].as_str().map(str::to_string),
                context_window: entry["context_window"].as_u64(),
            })
        })
        .collect();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_model_catalog() {
        let body = r#"{"models":[
            {"slug":"gpt-6.1-sol","display_name":"GPT-6.1 Sol","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}],"default_reasoning_level":"low"},
            {"slug":"internal","display_name":"Hidden","visibility":"hide"},
            {"slug":"plain"}
        ]}"#;
        let models = parse_model_catalog(body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].display_name, "GPT-6.1 Sol");
        assert_eq!(models[0].reasoning_levels, vec!["low", "high"]);
        assert_eq!(models[1].display_name, "plain");
    }

    #[test]
    fn stores_and_reloads_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatgpt-auth.dat");
        let accounts = ChatGptAccounts::open(&path);
        accounts
            .modify(|file| {
                file.accounts.insert(
                    "chatgpt".into(),
                    StoredAccount {
                        client_id: "oaiapp_1".into(),
                        email: Some("a@b.c".into()),
                        scopes: vec![oauth::USAGE_SHARING_SCOPE.into()],
                        tokens: Some(StoredTokens { access_token: "t".into(), refresh_token: None, expires_at: Utc::now() + chrono::Duration::hours(1), earliest_refresh_at: None }),
                        ..Default::default()
                    },
                );
            })
            .unwrap();
        let reopened = ChatGptAccounts::open(&path);
        let summary = reopened.account("chatgpt").unwrap();
        assert!(summary.usage_sharing);
        assert_eq!(summary.display_name(), "a@b.c");
        reopened.rename("chatgpt", "plan");
        assert!(reopened.account("chatgpt").is_none());
        assert!(reopened.view("plan").registered);
    }
}
