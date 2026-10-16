//! The OAuth side of Sign in with ChatGPT: PKCE, a loopback redirect on 127.0.0.1 and
//! the token endpoint. The first sign-in registers LLM Subs as a client for this
//! user; the issued client id is kept and reused from then on.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const REVOKE_URL: &str = "https://auth.openai.com/api/accounts/oauth/revoke";
/// The API the tokens are issued for.
pub const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// Granted when the user allows this app to use their ChatGPT plan.
pub const USAGE_SHARING_SCOPE: &str = "chatgpt.tokens.use.direct";
const OFFLINE_ACCESS_SCOPE: &str = "offline_access";
/// Client id that asks the authorization server to register a new client for this user.
pub const REGISTRATION_CLIENT_ID: &str = "dynamic_agent_client";
/// Shown to the user when they approve the connection.
pub const APP_NAME: &str = "LLM Subs";
const CALLBACK_PATH: &str = "/auth/callback";
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;

/// Errors from the token endpoint that mean the user has to sign in again.
const SIGN_IN_AGAIN_CODES: [&str; 6] =
    ["invalid_grant", "invalid_refresh_token", "token_expired", "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"];

/// 32 random bytes, base64url encoded: used for state, nonce and the PKCE verifier.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).expect("system random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn code_challenge(verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref())
}

pub fn is_valid_issued_client_id(client_id: &str) -> bool {
    client_id != REGISTRATION_CLIENT_ID
        && (1..=200).contains(&client_id.len())
        && client_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub struct AuthorizeOptions<'a> {
    /// The client id issued at the first sign-in; `None` registers a new client.
    pub client_id: Option<&'a str>,
    pub host_id: &'a str,
    pub login_hint: Option<&'a str>,
    /// Ask the user again whether LLM Subs may use their plan.
    pub reconsent: bool,
}

pub struct AuthorizationRequest {
    pub url: String,
    pub redirect_uri: String,
    pub state: String,
    pub nonce: String,
    pub verifier: String,
}

pub fn authorization_request(redirect_uri: &str, options: &AuthorizeOptions<'_>) -> AuthorizationRequest {
    let state = random_token();
    let nonce = random_token();
    let verifier = random_token();
    let challenge = code_challenge(&verifier);
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", options.client_id.unwrap_or(REGISTRATION_CLIENT_ID)),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPES),
        ("resource", RESOURCE),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
        ("ext_agent_host_id", options.host_id),
    ];
    if options.client_id.is_none() {
        params.push(("agent_name_hint", APP_NAME));
    }
    if let Some(email) = options.login_hint {
        params.push(("login_hint", email));
    }
    if options.reconsent {
        params.push(("prompt", "consent"));
    }
    let url = reqwest::Url::parse_with_params(AUTHORIZE_URL, &params).expect("static authorize URL").to_string();
    AuthorizationRequest { url, redirect_uri: redirect_uri.to_string(), state, nonce, verifier }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    Approved { code: String, client_id: Option<String> },
    Declined(String),
}

/// The local web server the browser is redirected to after the user approves.
pub struct LoopbackListener {
    listener: TcpListener,
    port: u16,
}

impl LoopbackListener {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("opening a local port for the sign-in callback")?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Serves requests until one carries the expected state. Stray requests (a
    /// favicon, a stale tab) are answered and ignored.
    pub async fn wait_for_callback(&self, expected_state: &str) -> Result<CallbackOutcome> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            match self.handle_connection(stream, expected_state).await {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => continue,
                Err(error) => tracing::debug!(%error, "ignoring malformed sign-in callback request"),
            }
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream, expected_state: &str) -> Result<Option<CallbackOutcome>> {
        let head = tokio::time::timeout(Duration::from_secs(10), read_request_head(&mut stream)).await.context("callback request timed out")??;
        let request = match parse_callback_request(&head, self.port, expected_state) {
            Ok(request) => request,
            Err(rejection) => {
                respond(&mut stream, rejection.status, &page(rejection.title, rejection.message)).await?;
                return Ok(None);
            }
        };
        let (title, message) = match &request {
            CallbackOutcome::Approved { .. } => ("Return to LLM Subs", "You are signed in. You can close this tab."),
            CallbackOutcome::Declined(_) => ("Sign-in not completed", "LLM Subs was not connected. You can close this tab."),
        };
        respond(&mut stream, "200 OK", &page(title, message)).await?;
        Ok(Some(request))
    }
}

struct Rejection {
    status: &'static str,
    title: &'static str,
    message: &'static str,
}

fn parse_callback_request(head: &str, port: u16, expected_state: &str) -> Result<CallbackOutcome, Rejection> {
    let not_found = Rejection { status: "404 Not Found", title: "Not found", message: "This page belongs to the LLM Subs sign-in." };
    let bad_request = Rejection { status: "400 Bad Request", title: "Sign-in link not recognised", message: "Start the sign-in again from LLM Subs." };
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else { return Err(bad_request) };
    if method != "GET" {
        return Err(Rejection { status: "405 Method Not Allowed", ..bad_request });
    }
    let host = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("host").then(|| value.trim().to_string())
    });
    if host.as_deref() != Some(format!("127.0.0.1:{port}").as_str()) {
        return Err(bad_request);
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1:{port}{target}")) else { return Err(bad_request) };
    if url.path() != CALLBACK_PATH {
        return Err(not_found);
    }
    let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let values = |key: &str| pairs.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
    let states = values("state");
    if states.len() != 1 || !constant_time_eq(states[0], expected_state) {
        return Err(bad_request);
    }
    if let Some(error) = values("error").first() {
        let description = values("error_description").first().map(|d| format!(": {d}")).unwrap_or_default();
        return Ok(CallbackOutcome::Declined(format!("{error}{description}")));
    }
    let Some(code) = values("code").first().filter(|c| !c.is_empty()).map(|c| c.to_string()) else { return Err(bad_request) };
    let client_id = values("client_id").first().map(|c| c.to_string());
    Ok(CallbackOutcome::Approved { code, client_id })
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |difference, (x, y)| difference | (x ^ y)) == 0
}

async fn read_request_head(stream: &mut TcpStream) -> Result<String> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&buffer[..read]);
        if head.len() > MAX_REQUEST_HEAD_BYTES {
            bail!("request head too large");
        }
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await.ok();
    Ok(())
}

fn page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
<style>body{{font-family:system-ui,sans-serif;display:grid;place-items:center;height:90vh;margin:0;color:#222;background:#f6f7fa}}\
@media (prefers-color-scheme:dark){{body{{color:#eee;background:#181a20}}}}main{{text-align:center}}</style></head>\
<body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    )
}

#[derive(Debug, Clone)]
pub enum TokenError {
    /// The grant is gone (expired, revoked, already used): the user must sign in again.
    SignInAgain(String),
    /// The server refused for another reason; retrying will not help.
    Rejected(String),
    /// Network trouble or a server error; worth retrying.
    Unavailable(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::SignInAgain(message) => write!(f, "the ChatGPT sign-in has expired ({message}); sign in again"),
            TokenError::Rejected(message) => write!(f, "ChatGPT sign-in was rejected: {message}"),
            TokenError::Unavailable(message) => write!(f, "could not reach the ChatGPT sign-in service: {message}"),
        }
    }
}

impl std::error::Error for TokenError {}

#[derive(Debug, Clone, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    token_type: String,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    earliest_refresh_at: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub earliest_refresh_at: Option<DateTime<Utc>>,
    /// `None` when the server did not restate the scopes (refresh keeps the old ones).
    pub scopes: Option<Vec<String>>,
}

pub async fn exchange_code(http: &reqwest::Client, client_id: &str, code: &str, verifier: &str, redirect_uri: &str) -> Result<TokenGrant, TokenError> {
    let grant = token_request(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let Some(scopes) = &grant.scopes else {
        return Err(TokenError::Rejected("the token response did not list the granted scopes".into()));
    };
    if scopes.iter().any(|s| s == OFFLINE_ACCESS_SCOPE) && grant.refresh_token.is_none() {
        return Err(TokenError::Rejected("the token response is missing the refresh token".into()));
    }
    Ok(grant)
}

pub async fn refresh(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<TokenGrant, TokenError> {
    token_request(http, &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh_token), ("resource", RESOURCE)]).await
}

/// Revokes the refresh token, retrying once on a server or network error. An
/// already invalid token counts as revoked.
pub async fn revoke(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<()> {
    let form = [("token", refresh_token), ("token_type_hint", "refresh_token"), ("client_id", client_id)];
    let mut last_error = None;
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        match form_post(http, REVOKE_URL, &form).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) if response.status().is_server_error() => last_error = Some(anyhow!("HTTP {}", response.status())),
            Ok(response) => return Err(anyhow!("HTTP {}", response.status())),
            Err(error) => last_error = Some(error.into()),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("revocation failed")))
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<TokenGrant, TokenError> {
    let response = form_post(http, TOKEN_URL, form).send().await.map_err(|e| TokenError::Unavailable(e.to_string()))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| TokenError::Unavailable(e.to_string()))?;
    if !status.is_success() {
        let (code, message) = token_error_details(&body);
        return Err(if SIGN_IN_AGAIN_CODES.contains(&code.as_str()) {
            TokenError::SignInAgain(message)
        } else if status.is_server_error() {
            TokenError::Unavailable(format!("HTTP {status}: {message}"))
        } else {
            TokenError::Rejected(message)
        });
    }
    parse_token_response(&body).map_err(|e| TokenError::Rejected(format!("{e:#}")))
}

fn form_post(http: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form).finish();
    http.post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(body)
}

/// The error code (`invalid_grant`, ...) and a readable message from an error body.
fn token_error_details(body: &str) -> (String, String) {
    let Ok(value) = serde_json::from_str::<Value>(body) else { return (String::new(), body.chars().take(300).collect()) };
    let error = match &value["error"] {
        Value::Object(_) => value["error"].clone(),
        _ => value.clone(),
    };
    let code = match &value["error"] {
        Value::String(code) => code.clone(),
        _ => error["code"].as_str().unwrap_or_default().to_string(),
    };
    let message = value["error_description"]
        .as_str()
        .or_else(|| error["message"].as_str())
        .or_else(|| value["detail"].as_str())
        .unwrap_or(code.as_str())
        .to_string();
    (code, message)
}

fn parse_token_response(body: &str) -> Result<TokenGrant> {
    let raw: RawTokenResponse = serde_json::from_str(body).context("unreadable token response")?;
    if !raw.token_type.eq_ignore_ascii_case("bearer") {
        bail!("unexpected token type '{}'", raw.token_type);
    }
    if raw.expires_in <= 0 {
        bail!("the access token is already expired");
    }
    let earliest_refresh_at = match &raw.earliest_refresh_at {
        Some(Value::Number(seconds)) => seconds.as_i64().and_then(|s| Utc.timestamp_opt(s, 0).single()),
        Some(Value::String(text)) => DateTime::parse_from_rfc3339(text).ok().map(|t| t.with_timezone(&Utc)),
        _ => None,
    };
    Ok(TokenGrant {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        id_token: raw.id_token,
        expires_at: Utc::now() + chrono::Duration::seconds(raw.expires_in),
        earliest_refresh_at,
        scopes: raw.scope.map(|scope| scope.split_whitespace().map(str::to_string).collect()),
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IdTokenClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    iss: String,
    #[serde(default)]
    aud: Value,
    #[serde(default)]
    exp: i64,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    azp: Option<String>,
}

/// Reads the identity from an ID token and checks that it was issued to this client
/// for this sign-in. The token arrives straight from the token endpoint over TLS,
/// which OpenID Connect accepts in place of checking the signature.
pub fn read_id_token(id_token: &str, client_id: &str, expected_nonce: Option<&str>) -> Result<IdTokenClaims> {
    let payload = id_token.split('.').nth(1).context("malformed ID token")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("malformed ID token payload")?;
    let claims: IdTokenClaims = serde_json::from_slice(&bytes).context("unreadable ID token claims")?;
    if claims.iss != ISSUER {
        bail!("ID token from an unexpected issuer '{}'", claims.iss);
    }
    let audience_matches = match &claims.aud {
        Value::String(audience) => audience == client_id,
        Value::Array(audiences) => audiences.iter().any(|a| a.as_str() == Some(client_id)),
        _ => false,
    };
    if !audience_matches || claims.azp.as_deref().is_some_and(|azp| azp != client_id) {
        bail!("ID token was issued to another client");
    }
    if claims.exp + 5 < Utc::now().timestamp() {
        bail!("ID token has expired");
    }
    if let Some(expected) = expected_nonce
        && claims.nonce.as_deref() != Some(expected)
    {
        bail!("ID token does not belong to this sign-in");
    }
    if claims.sub.is_empty() {
        bail!("ID token has no subject");
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_7636_example() {
        assert_eq!(code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn first_sign_in_registers_and_later_ones_reuse_the_client() {
        let options = AuthorizeOptions { client_id: None, host_id: "urn:uuid:1", login_hint: None, reconsent: false };
        let first = authorization_request("http://127.0.0.1:5000/auth/callback", &options);
        let url = reqwest::Url::parse(&first.url).unwrap();
        let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        assert_eq!(param("client_id").as_deref(), Some(REGISTRATION_CLIENT_ID));
        assert_eq!(param("agent_name_hint").as_deref(), Some(APP_NAME));
        assert_eq!(param("resource").as_deref(), Some(RESOURCE));
        assert_eq!(param("code_challenge").unwrap(), code_challenge(&first.verifier));
        assert_eq!(param("state").unwrap(), first.state);

        let options = AuthorizeOptions { client_id: Some("oaiapp_123"), host_id: "urn:uuid:1", login_hint: Some("a@b.c"), reconsent: true };
        let again = reqwest::Url::parse(&authorization_request("http://127.0.0.1:5000/auth/callback", &options).url).unwrap();
        let names: Vec<String> = again.query_pairs().map(|(k, _)| k.into_owned()).collect();
        assert!(!names.contains(&"agent_name_hint".to_string()));
        assert!(names.contains(&"login_hint".to_string()) && names.contains(&"prompt".to_string()));
    }

    #[test]
    fn callback_requires_matching_state_host_and_path() {
        let request = |target: &str, host: &str| format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n");
        let approved = parse_callback_request(&request("/auth/callback?code=abc&state=s1&client_id=oaiapp_9", "127.0.0.1:4000"), 4000, "s1");
        assert_eq!(approved.ok(), Some(CallbackOutcome::Approved { code: "abc".into(), client_id: Some("oaiapp_9".into()) }));
        assert!(parse_callback_request(&request("/auth/callback?code=abc&state=s2", "127.0.0.1:4000"), 4000, "s1").is_err());
        assert!(parse_callback_request(&request("/auth/callback?code=abc&state=s1&state=s1", "127.0.0.1:4000"), 4000, "s1").is_err());
        assert!(parse_callback_request(&request("/auth/callback?code=abc&state=s1", "localhost:4000"), 4000, "s1").is_err());
        assert!(parse_callback_request(&request("/favicon.ico", "127.0.0.1:4000"), 4000, "s1").is_err());
        let declined = parse_callback_request(&request("/auth/callback?error=access_denied&state=s1", "127.0.0.1:4000"), 4000, "s1");
        assert_eq!(declined.ok(), Some(CallbackOutcome::Declined("access_denied".into())));
    }

    #[test]
    fn loopback_listener_ignores_strays_and_answers_the_browser() {
        crate::llm::install_crypto_provider();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let listener = LoopbackListener::bind().await.unwrap();
            let redirect_uri = listener.redirect_uri();
            assert!(redirect_uri.starts_with("http://127.0.0.1:") && redirect_uri.ends_with(CALLBACK_PATH));
            let browser = tokio::spawn(async move {
                let http = reqwest::Client::new();
                let stray = http.get(format!("{redirect_uri}?code=x&state=wrong")).send().await.unwrap();
                let approved = http.get(format!("{redirect_uri}?code=the-code&state=expected&client_id=oaiapp_7")).send().await.unwrap();
                (stray.status().as_u16(), approved.status().as_u16(), approved.text().await.unwrap())
            });
            let outcome = listener.wait_for_callback("expected").await.unwrap();
            assert_eq!(outcome, CallbackOutcome::Approved { code: "the-code".into(), client_id: Some("oaiapp_7".into()) });
            let (stray_status, approved_status, page) = browser.await.unwrap();
            assert_eq!((stray_status, approved_status), (400, 200));
            assert!(page.contains("Return to LLM Subs"));
        });
    }

    #[test]
    fn validates_issued_client_ids() {
        assert!(is_valid_issued_client_id("oaiapp_AbC-1"));
        assert!(!is_valid_issued_client_id(REGISTRATION_CLIENT_ID));
        assert!(!is_valid_issued_client_id("bad id"));
        assert!(!is_valid_issued_client_id(""));
    }

    #[test]
    fn reads_token_responses_and_errors() {
        let grant = parse_token_response(
            r#"{"access_token":"a","refresh_token":"r","id_token":"i","token_type":"Bearer","expires_in":3600,"scope":"openid chatgpt.tokens.use.direct","earliest_refresh_at":1700000000}"#,
        )
        .unwrap();
        assert_eq!(grant.scopes.unwrap(), vec!["openid", USAGE_SHARING_SCOPE]);
        assert_eq!(grant.earliest_refresh_at.unwrap().timestamp(), 1_700_000_000);
        assert!(parse_token_response(r#"{"access_token":"a","token_type":"mac","expires_in":3600}"#).is_err());
        assert_eq!(token_error_details(r#"{"error":"invalid_grant","error_description":"expired"}"#), ("invalid_grant".into(), "expired".into()));
        assert_eq!(token_error_details(r#"{"error":{"code":"invalid_client","message":"nope"}}"#), ("invalid_client".into(), "nope".into()));
    }

    #[test]
    fn checks_id_token_claims() {
        let encode = |claims: &str| format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims));
        let future = Utc::now().timestamp() + 600;
        let good = encode(&format!(r#"{{"iss":"{ISSUER}","aud":"oaiapp_1","exp":{future},"sub":"user-1","email":"a@b.c","nonce":"n1"}}"#));
        let claims = read_id_token(&good, "oaiapp_1", Some("n1")).unwrap();
        assert_eq!(claims.email.as_deref(), Some("a@b.c"));
        assert!(read_id_token(&good, "oaiapp_2", Some("n1")).is_err());
        assert!(read_id_token(&good, "oaiapp_1", Some("other")).is_err());
        assert!(read_id_token(&good, "oaiapp_1", None).is_ok());
    }
}
