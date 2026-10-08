//! Sign-in with the OAuth device code flow, token refresh and the per-account token file.
//!
//! Tokens live in `accounts/<id>/token.json` (mode 0600), shared by app and CLI. Sign-ins
//! from `ms365-mail` can be imported: same app registration, so its refresh tokens work here.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::config::{AccountConfig, Config};
use crate::error::{Error, ErrorCode, Result};
use crate::paths;

/// Same scopes ms365-mail consented to, so imported refresh tokens cover them.
pub const SCOPES: &str = "offline_access openid profile https://graph.microsoft.com/Mail.Read \
https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/User.Read";

const AUTHORITY: &str = "https://login.microsoftonline.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    #[serde(default)]
    pub tenant: Option<String>,
}

impl Token {
    pub fn load(account_id: &str) -> Result<Self> {
        let path = paths::token_file(account_id);
        let text = std::fs::read_to_string(&path).map_err(|_| {
            Error::auth(format!("account '{account_id}' is not signed in"))
                .hint(format!("run `jm accounts add` (or `jm accounts import`); expected {}", path.display()))
        })?;
        serde_json::from_str(&text).map_err(|e| Error::auth(format!("token file {} is damaged: {e}", path.display())))
    }

    pub fn save(&self, account_id: &str) -> Result<()> {
        paths::write_private(&paths::token_file(account_id), &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn is_fresh(&self) -> bool {
        self.expires_at > now() + 120
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: String,
}

fn token_endpoint(tenant: Option<&str>) -> String {
    format!("{AUTHORITY}/{}/oauth2/v2.0/token", tenant.unwrap_or("common"))
}

fn post_form(url: &str, form: &[(&str, &str)]) -> std::result::Result<TokenResponse, OAuthError> {
    match ureq::post(url).timeout(Duration::from_secs(30)).send_form(form) {
        Ok(resp) => resp.into_json::<TokenResponse>().map_err(|e| OAuthError {
            error: "invalid_response".into(),
            error_description: e.to_string(),
        }),
        Err(ureq::Error::Status(_, resp)) => Err(resp.into_json::<OAuthError>().unwrap_or(OAuthError {
            error: "http_error".into(),
            error_description: "the sign-in service answered with an error".into(),
        })),
        Err(e) => Err(OAuthError { error: "network".into(), error_description: e.to_string() }),
    }
}

/// Exchange the refresh token for a new access token and store the result.
pub fn refresh(account_id: &str, client_id: &str, token: &Token) -> Result<Token> {
    let form = [
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", token.refresh_token.as_str()),
        ("scope", SCOPES),
    ];
    let resp = post_form(&token_endpoint(token.tenant.as_deref()), &form).map_err(|e| {
        let code = if e.error == "network" { ErrorCode::Network } else { ErrorCode::Auth };
        Error::new(code, format!("could not refresh the sign-in: {} {}", e.error, first_line(&e.error_description)))
            .hint("sign in again with `jm accounts add` or in Just Mail")
    })?;
    let fresh = Token {
        access_token: resp.access_token,
        refresh_token: resp.refresh_token.unwrap_or_else(|| token.refresh_token.clone()),
        expires_at: now() + resp.expires_in,
        tenant: token.tenant.clone(),
    };
    fresh.save(account_id)?;
    Ok(fresh)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// A started device code sign-in: show `user_code` and `verification_uri` to the person.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub message: String,
}

fn default_interval() -> u64 {
    5
}

pub fn start_device_code(client_id: &str) -> Result<DeviceCode> {
    let url = format!("{AUTHORITY}/common/oauth2/v2.0/devicecode");
    ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .send_form(&[("client_id", client_id), ("scope", SCOPES)])
        .map_err(|e| Error::new(ErrorCode::Network, format!("could not start the sign-in: {e}")))?
        .into_json()
        .map_err(|e| Error::general(format!("unexpected sign-in answer: {e}")))
}

/// Outcome of one poll of a device code sign-in.
pub enum Poll {
    Pending,
    Done(Token),
}

pub fn poll_device_code(client_id: &str, code: &DeviceCode) -> Result<Poll> {
    let form = [
        ("client_id", client_id),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", code.device_code.as_str()),
    ];
    match post_form(&token_endpoint(None), &form) {
        Ok(resp) => {
            let tenant = resp.id_token.as_deref().and_then(|t| jwt_claim(t, "tid"));
            Ok(Poll::Done(Token {
                access_token: resp.access_token,
                refresh_token: resp.refresh_token.ok_or_else(|| Error::auth("sign-in returned no refresh token"))?,
                expires_at: now() + resp.expires_in,
                tenant,
            }))
        }
        Err(e) if e.error == "authorization_pending" || e.error == "slow_down" => Ok(Poll::Pending),
        Err(e) if e.error == "network" => Ok(Poll::Pending),
        Err(e) => Err(Error::auth(format!("sign-in failed: {} {}", e.error, first_line(&e.error_description)))),
    }
}

/// Block until the person finished the device code sign-in (or it expired).
pub fn wait_device_code(client_id: &str, code: &DeviceCode) -> Result<Token> {
    let deadline = now() + code.expires_in;
    loop {
        std::thread::sleep(Duration::from_secs(code.interval.max(2)));
        if let Poll::Done(token) = poll_device_code(client_id, code)? {
            return Ok(token);
        }
        if now() > deadline {
            return Err(Error::auth("the sign-in code expired").hint("start the sign-in again"));
        }
    }
}

/// Read one string claim from a JWT without verifying it (only used for display data).
pub fn jwt_claim(jwt: &str, claim: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get(claim)?.as_str().map(str::to_string)
}

/// Store a fresh token for the signed-in mailbox and register the account in the config.
pub fn register_account(config: &mut Config, token: Token) -> Result<AccountConfig> {
    let me = crate::graph::fetch_me(&token.access_token)?;
    let email = me.email();
    let id = config.new_account_id(&email);
    token.save(&id)?;
    let account = AccountConfig {
        id: id.clone(),
        email,
        name: me.display_name.unwrap_or_default(),
        signature: String::new(),
        read_only: false,
    };
    config.upsert(account.clone());
    config.save()?;
    Ok(config.account(Some(&id))?.clone())
}

/// A sign-in found in ms365-mail's token caches.
#[derive(Debug, Clone, Serialize)]
pub struct Ms365Login {
    pub username: String,
    pub profile: String,
    #[serde(skip)]
    refresh_token: String,
    #[serde(skip)]
    tenant: Option<String>,
}

fn ms365_caches() -> Vec<(String, PathBuf)> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let base = home.join(".config").join("ms365-mail");
    let mut caches = vec![("default".to_string(), base.join("token-cache.json"))];
    if let Ok(entries) = std::fs::read_dir(base.join("profiles")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            caches.push((name, entry.path().join("token-cache.json")));
        }
    }
    caches
}

/// Sign-ins of ms365-mail (default profile and `profiles/*`) that use our app registration.
pub fn find_ms365_logins(client_id: &str) -> Vec<Ms365Login> {
    let mut found = Vec::new();
    for (profile, path) in ms365_caches() {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(cache) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let accounts = cache.get("Account").and_then(|v| v.as_object());
        let Some(tokens) = cache.get("RefreshToken").and_then(|v| v.as_object()) else { continue };
        for rt in tokens.values() {
            if rt.get("client_id").and_then(|v| v.as_str()) != Some(client_id) {
                continue;
            }
            let Some(secret) = rt.get("secret").and_then(|v| v.as_str()) else { continue };
            let home_account = rt.get("home_account_id").and_then(|v| v.as_str()).unwrap_or_default();
            let account = accounts.and_then(|a| {
                a.values().find(|acc| acc.get("home_account_id").and_then(|v| v.as_str()) == Some(home_account))
            });
            let username = account
                .and_then(|a| a.get("username"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let tenant = account.and_then(|a| a.get("realm")).and_then(|v| v.as_str()).map(str::to_string);
            found.push(Ms365Login { username, profile: profile.clone(), refresh_token: secret.to_string(), tenant });
        }
    }
    found
}

/// Turn an ms365-mail sign-in into a Just Mail account (refreshes once to prove it works).
pub fn import_ms365_login(config: &mut Config, login: &Ms365Login) -> Result<AccountConfig> {
    let seed = Token {
        access_token: String::new(),
        refresh_token: login.refresh_token.clone(),
        expires_at: 0,
        tenant: login.tenant.clone(),
    };
    // Refresh into a scratch id first: the real id is only known after asking Graph who this is.
    let scratch = format!(".import-{}", std::process::id());
    let token = refresh(&scratch, config.client_id(), &seed);
    let _ = std::fs::remove_dir_all(paths::account_dir(&scratch));
    register_account(config, token?)
}
