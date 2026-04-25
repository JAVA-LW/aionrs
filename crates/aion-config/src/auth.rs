// Provider-scoped OAuth credential storage plus device authorization flow helpers.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const LEGACY_PROVIDER_ID: &str = "anthropic";
const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CHATGPT_ISSUER: &str = "https://auth.openai.com";
const CHATGPT_REDIRECT_URL: &str = "http://localhost:1455/auth/callback";
const COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
const COPILOT_AUTH_URL: &str = "https://github.com/login";
const COPILOT_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_API_BASE_URL: &str = "https://api.githubcopilot.com";
const COPILOT_MODEL_DISCOVERY_PATH: &str = "/models";
const COPILOT_TOKEN_FALLBACK_SECS: u64 = 60 * 60 * 24 * 365 * 10;
const CALLBACK_TIMEOUT_SECS: u64 = 300;
const OAUTH_POLLING_SAFETY_MARGIN_MS: u64 = 3000;
const OAUTH_USER_AGENT: &str = concat!("aionrs/", env!("CARGO_PKG_VERSION"));

/// Token bundle persisted for OAuth-based auth modes such as Claude or ChatGPT.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// Stored OAuth credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCredentials {
    pub auth_mode: Option<String>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub tokens: OAuthTokens,
    pub expires_at: DateTime<Utc>,
    pub token_type: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OAuthCredentialsDisk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh: Option<DateTime<Utc>>,
    tokens: OAuthTokens,
    expires_at: DateTime<Utc>,
    token_type: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct OAuthCredentialsFlat {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    expires_at: DateTime<Utc>,
    token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OAuthCredentialsSerde {
    Disk(OAuthCredentialsDisk),
    Flat(OAuthCredentialsFlat),
}

impl From<OAuthCredentialsDisk> for OAuthCredentials {
    fn from(value: OAuthCredentialsDisk) -> Self {
        Self {
            auth_mode: value.auth_mode,
            last_refresh: value.last_refresh,
            tokens: value.tokens,
            expires_at: value.expires_at,
            token_type: value.token_type,
            metadata: value.metadata,
        }
    }
}

impl From<OAuthCredentialsFlat> for OAuthCredentials {
    fn from(value: OAuthCredentialsFlat) -> Self {
        Self {
            auth_mode: value.auth_mode,
            last_refresh: value.last_refresh,
            tokens: OAuthTokens {
                access_token: value.access_token,
                refresh_token: value.refresh_token,
                id_token: value.id_token,
                account_id: value.account_id,
            },
            expires_at: value.expires_at,
            token_type: value.token_type,
            metadata: value.metadata,
        }
    }
}

impl From<&OAuthCredentials> for OAuthCredentialsDisk {
    fn from(value: &OAuthCredentials) -> Self {
        Self {
            auth_mode: value.auth_mode.clone(),
            last_refresh: value.last_refresh,
            tokens: value.tokens.clone(),
            expires_at: value.expires_at,
            token_type: value.token_type.clone(),
            metadata: value.metadata.clone(),
        }
    }
}

impl Serialize for OAuthCredentials {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        OAuthCredentialsDisk::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OAuthCredentials {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let parsed = OAuthCredentialsSerde::deserialize(deserializer)?;
        Ok(match parsed {
            OAuthCredentialsSerde::Disk(disk) => disk.into(),
            OAuthCredentialsSerde::Flat(flat) => flat.into(),
        })
    }
}

/// Stored API key credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyCredentials {
    pub key: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Exact persisted ChatGPT auth shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatgptTokens {
    pub access_token: String,
    pub account_id: String,
    pub id_token: String,
}

/// Exact persisted ChatGPT auth shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatgptAuth {
    #[serde(rename = "OPENAI_API_KEY")]
    pub openai_api_key: Option<String>,
    pub auth_mode: String,
    pub last_refresh: String,
    pub tokens: ChatgptTokens,
}

/// Stored auth entry for one provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum StoredAuth {
    Chatgpt(ChatgptAuth),
    OAuth {
        #[serde(rename = "type")]
        kind: OAuthAuthKind,
        #[serde(flatten)]
        credentials: OAuthCredentials,
    },
    ApiKey {
        #[serde(rename = "type")]
        kind: ApiAuthKind,
        #[serde(flatten)]
        credentials: ApiKeyCredentials,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OAuthAuthKind {
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApiAuthKind {
    #[serde(rename = "api_key")]
    ApiKey,
}

impl StoredAuth {
    fn oauth(credentials: OAuthCredentials) -> Self {
        Self::OAuth {
            kind: OAuthAuthKind::OAuth,
            credentials,
        }
    }

    pub fn chatgpt(auth: ChatgptAuth) -> Self {
        Self::Chatgpt(auth)
    }
}

/// Provider-scoped auth store persisted in `<config_dir>/aionrs/auth.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthStore {
    #[serde(default)]
    pub providers: HashMap<String, StoredAuth>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StoredAuthFile {
    LegacyOAuth(OAuthCredentials),
    Store(AuthStore),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChatgptPrivateAuth {
    refresh_token: String,
    expires_at: DateTime<Utc>,
    token_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct PrivateAuthStore {
    #[serde(default)]
    providers: HashMap<String, ChatgptPrivateAuth>,
}

/// OAuth device code response.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// OAuth token response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}

/// OAuth token error response (during polling).
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct PollingTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OAuthRequestEncoding {
    #[default]
    Form,
    Json,
}

/// Config for OAuth endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_url")]
    pub auth_url: String,
    #[serde(default = "default_token_url")]
    pub token_url: String,
    #[serde(default = "default_client_id")]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub flow: AuthFlow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub authorize_params: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_discovery_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_path: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub api_headers: HashMap<String, String>,
    #[serde(default)]
    pub use_responses_api: bool,
    #[serde(default)]
    pub system_as_instructions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_expires_in_fallback_secs: Option<u64>,
    #[serde(default)]
    pub request_encoding: OAuthRequestEncoding,
    #[serde(default)]
    pub http1_only: bool,
    #[serde(default)]
    pub disable_connection_reuse: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlow {
    #[default]
    DeviceCode,
    BrowserPkce,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            auth_url: default_auth_url(),
            token_url: default_token_url(),
            client_id: default_client_id(),
            auth_mode: None,
            flow: AuthFlow::DeviceCode,
            redirect_url: None,
            scope: None,
            authorize_params: HashMap::new(),
            api_base_url: None,
            api_path: None,
            model_discovery_path: None,
            usage_path: None,
            api_headers: HashMap::new(),
            use_responses_api: false,
            system_as_instructions: false,
            account_id_header: None,
            token_expires_in_fallback_secs: None,
            request_encoding: OAuthRequestEncoding::Form,
            http1_only: false,
            disable_connection_reuse: false,
        }
    }
}

impl AuthConfig {
    /// Built-in defaults for providers that currently support CLI login.
    pub fn for_provider(provider_id: &str) -> anyhow::Result<Self> {
        match provider_id {
            "anthropic" => Ok(Self::default()),
            "chatgpt" | "openai" => Ok(Self {
                auth_url: format!("{}/oauth/authorize", CHATGPT_ISSUER),
                token_url: format!("{}/oauth/token", CHATGPT_ISSUER),
                client_id: CHATGPT_CLIENT_ID.to_string(),
                auth_mode: Some("chatgpt".to_string()),
                flow: AuthFlow::BrowserPkce,
                redirect_url: Some(CHATGPT_REDIRECT_URL.to_string()),
                scope: Some("openid profile email offline_access".to_string()),
                authorize_params: HashMap::from([
                    ("id_token_add_organizations".to_string(), "true".to_string()),
                    ("codex_cli_simplified_flow".to_string(), "true".to_string()),
                    ("originator".to_string(), "aionrs".to_string()),
                ]),
                api_base_url: Some("https://chatgpt.com".to_string()),
                api_path: Some("/backend-api/codex/responses".to_string()),
                model_discovery_path: Some("/backend-api/codex/models".to_string()),
                usage_path: Some("/backend-api/wham/usage".to_string()),
                api_headers: HashMap::from([
                    ("Accept-Encoding".to_string(), "identity".to_string()),
                    ("originator".to_string(), "aionrs".to_string()),
                    ("User-Agent".to_string(), OAUTH_USER_AGENT.to_string()),
                ]),
                use_responses_api: true,
                system_as_instructions: true,
                account_id_header: Some("ChatGPT-Account-Id".to_string()),
                token_expires_in_fallback_secs: None,
                request_encoding: OAuthRequestEncoding::Form,
                http1_only: true,
                disable_connection_reuse: true,
            }),
            "copilot" | "github-copilot" => Ok(Self {
                auth_url: COPILOT_AUTH_URL.to_string(),
                token_url: COPILOT_TOKEN_URL.to_string(),
                client_id: COPILOT_CLIENT_ID.to_string(),
                auth_mode: Some("copilot".to_string()),
                flow: AuthFlow::DeviceCode,
                redirect_url: None,
                scope: Some("read:user".to_string()),
                authorize_params: HashMap::new(),
                api_base_url: Some(COPILOT_API_BASE_URL.to_string()),
                api_path: Some("/chat/completions".to_string()),
                model_discovery_path: Some(COPILOT_MODEL_DISCOVERY_PATH.to_string()),
                usage_path: None,
                api_headers: HashMap::from([
                    (
                        "User-Agent".to_string(),
                        format!("aionrs/{}", env!("CARGO_PKG_VERSION")),
                    ),
                    (
                        "Openai-Intent".to_string(),
                        "conversation-edits".to_string(),
                    ),
                    ("x-initiator".to_string(), "agent".to_string()),
                ]),
                use_responses_api: false,
                system_as_instructions: false,
                account_id_header: None,
                token_expires_in_fallback_secs: Some(COPILOT_TOKEN_FALLBACK_SECS),
                request_encoding: OAuthRequestEncoding::Json,
                http1_only: true,
                disable_connection_reuse: true,
            }),
            other => anyhow::bail!(
                "OAuth login for provider '{}' is not implemented yet. \
                 Configure [auth] manually or keep using API keys for now.",
                other
            ),
        }
    }
}

fn default_auth_url() -> String {
    "https://claude.ai/oauth".to_string()
}

fn default_token_url() -> String {
    "https://claude.ai/oauth/token".to_string()
}

fn default_client_id() -> String {
    "aionrs".to_string()
}

pub struct OAuthManager {
    client: reqwest::Client,
    config: AuthConfig,
    provider_id: String,
    credentials_path: PathBuf,
    private_credentials_path: PathBuf,
}

impl OAuthManager {
    pub fn new(provider_id: impl Into<String>, config: AuthConfig) -> Self {
        let auth_dir = crate::config::app_config_dir().unwrap_or_else(|| PathBuf::from("aionrs"));
        let credentials_path = auth_dir.join("auth.json");
        let private_credentials_path = auth_dir.join("auth-private.json");
        let mut client_builder = reqwest::Client::builder();
        if config.http1_only {
            client_builder = client_builder.http1_only();
        }
        if config.disable_connection_reuse {
            client_builder = client_builder.pool_max_idle_per_host(0);
        }
        let client = client_builder
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            config,
            provider_id: provider_id.into(),
            credentials_path,
            private_credentials_path,
        }
    }

    /// Full authorization flow for the configured provider.
    pub async fn login(&self) -> anyhow::Result<OAuthCredentials> {
        match self.config.flow {
            AuthFlow::DeviceCode => self.login_device_code().await,
            AuthFlow::BrowserPkce => self.login_browser_pkce().await,
        }
    }

    async fn login_device_code(&self) -> anyhow::Result<OAuthCredentials> {
        // Step 1: Request device code
        let device_code_url = format!("{}/device/code", self.config.auth_url);
        let resp = self
            .oauth_post(
                &device_code_url,
                vec![
                    ("client_id".to_string(), self.config.client_id.clone()),
                    (
                        "scope".to_string(),
                        self.config
                            .scope
                            .clone()
                            .unwrap_or_else(|| "user:inference".to_string()),
                    ),
                ],
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to request device code: {}", body);
        }

        let device_resp: DeviceCodeResponse = resp.json().await?;

        // Step 2: Display instructions
        eprintln!();
        eprintln!("  To authenticate provider '{}', visit:", self.provider_id);
        eprintln!("  {}", device_resp.verification_uri);
        eprintln!();
        eprintln!("  Enter code: {}", device_resp.user_code);
        eprintln!();
        eprintln!("  Waiting for authorization...");

        // Step 3: Poll for token
        let mut poll_interval_secs = device_resp.interval.max(5);
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(device_resp.expires_in);

        loop {
            if std::time::Instant::now() > deadline {
                anyhow::bail!("Device authorization timed out. Please try again.");
            }

            tokio::time::sleep(
                std::time::Duration::from_secs(poll_interval_secs)
                    + std::time::Duration::from_millis(OAUTH_POLLING_SAFETY_MARGIN_MS),
            )
            .await;

            let token_resp = self
                .oauth_post(
                    &self.config.token_url,
                    vec![
                        ("client_id".to_string(), self.config.client_id.clone()),
                        ("device_code".to_string(), device_resp.device_code.clone()),
                        (
                            "grant_type".to_string(),
                            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                        ),
                    ],
                )
                .send()
                .await;

            let token_resp = match token_resp {
                Ok(resp) => resp,
                Err(err) => {
                    eprintln!(
                        "  Temporary error while polling OAuth token endpoint: {}. Retrying ...",
                        err
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            let status = token_resp.status();
            let body = token_resp.text().await.unwrap_or_default();

            if let Ok(err_resp) = serde_json::from_str::<TokenErrorResponse>(&body) {
                match err_resp.error.as_str() {
                    "authorization_pending" => continue,
                    "slow_down" => {
                        poll_interval_secs = err_resp
                            .interval
                            .filter(|interval| *interval > 0)
                            .unwrap_or_else(|| poll_interval_secs.saturating_add(5));
                        continue;
                    }
                    "expired_token" => {
                        anyhow::bail!("Device code expired. Please try again.");
                    }
                    "access_denied" => {
                        anyhow::bail!("Authorization denied by user.");
                    }
                    other => {
                        let detail = err_resp
                            .error_description
                            .as_deref()
                            .map(|description| format!(" ({})", description))
                            .unwrap_or_default();
                        anyhow::bail!("OAuth error: {}{}", other, detail);
                    }
                }
            }

            if status.is_success() {
                let token: PollingTokenResponse = serde_json::from_str(&body).unwrap_or_default();
                let Some(access_token) = token.access_token.filter(|token| !token.is_empty())
                else {
                    eprintln!(
                        "  OAuth token endpoint returned a temporary success payload without an access token. Retrying ..."
                    );
                    continue;
                };
                let refreshed_at = Utc::now();
                let token = TokenResponse {
                    access_token,
                    refresh_token: token.refresh_token,
                    id_token: token.id_token,
                    expires_in: token.expires_in,
                    token_type: token.token_type.unwrap_or_else(|| "Bearer".to_string()),
                };
                let expires_at = self.expires_at_from_token(&token, refreshed_at);
                let account_id = extract_account_id(&token);
                let credentials = OAuthCredentials {
                    auth_mode: Some(self.effective_auth_mode()),
                    last_refresh: Some(refreshed_at),
                    tokens: OAuthTokens {
                        access_token: token.access_token,
                        refresh_token: token.refresh_token.clone(),
                        id_token: token.id_token.clone(),
                        account_id,
                    },
                    expires_at,
                    token_type: token.token_type,
                    metadata: HashMap::new(),
                };
                self.save_credentials(&credentials)?;
                return Ok(credentials);
            }

            anyhow::bail!("Unexpected OAuth response: {}", body);
        }
    }

    async fn login_browser_pkce(&self) -> anyhow::Result<OAuthCredentials> {
        let redirect_url = self
            .config
            .redirect_url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing redirect_url for browser PKCE flow"))?;
        let (code_verifier, code_challenge) = generate_pkce_pair();
        let state = random_base64url(32);
        let authorize_url =
            build_authorize_url(&self.config, &redirect_url, &code_challenge, &state)?;

        eprintln!();
        eprintln!("  Authentication URL for provider '{}':", self.provider_id);
        eprintln!("  {}", authorize_url);
        eprintln!();
        if let Err(err) = crate::shell::open_in_browser(&authorize_url) {
            eprintln!("  Failed to auto-open browser: {}", err);
            eprintln!("  You can still open the URL manually.");
            eprintln!();
        } else {
            eprintln!("  Your default browser should open automatically.");
            eprintln!("  If it does not, copy the URL above into your browser.");
            eprintln!();
        }
        eprintln!(
            "  Waiting for authorization callback on {} ...",
            redirect_url
        );
        eprintln!("  This command will stay open until authentication succeeds or times out.");

        let code = wait_for_browser_callback(&redirect_url, &state).await?;
        eprintln!("  Authorization callback received. Finishing login ...");
        let token = self
            .exchange_browser_code_for_tokens(&code, &redirect_url, &code_verifier)
            .await?;
        let (public_auth, private_auth, credentials) = self.chatgpt_auth_from_token(token)?;
        self.save_chatgpt_auth(&public_auth, &private_auth)?;
        Ok(credentials)
    }

    /// Get valid OAuth credentials (refresh if expired).
    pub async fn get_credentials(&self) -> anyhow::Result<OAuthCredentials> {
        match self.load_stored_auth()? {
            StoredAuth::OAuth { credentials, .. } => {
                self.ensure_oauth_credentials(credentials).await
            }
            StoredAuth::Chatgpt(auth) => self.ensure_chatgpt_credentials(auth).await,
            StoredAuth::ApiKey { .. } => anyhow::bail!(
                "Stored credentials for provider '{}' are API-key based, not OAuth.",
                self.provider_id
            ),
        }
    }

    /// Get a valid access token (refresh if expired).
    pub async fn get_token(&self) -> anyhow::Result<String> {
        Ok(self.get_credentials().await?.tokens.access_token)
    }

    async fn ensure_oauth_credentials(
        &self,
        creds: OAuthCredentials,
    ) -> anyhow::Result<OAuthCredentials> {
        if creds.expires_at > Utc::now() + chrono::Duration::minutes(1) {
            return Ok(creds);
        }

        if let Some(refresh_token) = &creds.tokens.refresh_token {
            let new_creds = self.refresh(refresh_token).await?;
            self.save_credentials(&new_creds)?;
            return Ok(new_creds);
        }

        anyhow::bail!(
            "Token for provider '{}' expired and no refresh token is available. \
             Run 'aionrs --provider {} --login'.",
            self.provider_id,
            self.provider_id
        )
    }

    async fn ensure_chatgpt_credentials(
        &self,
        auth: ChatgptAuth,
    ) -> anyhow::Result<OAuthCredentials> {
        let private = self.load_chatgpt_private()?;
        if let Some(private) = private {
            if private.expires_at > Utc::now() + chrono::Duration::minutes(1) {
                return Ok(chatgpt_to_oauth_credentials(&auth, Some(&private)));
            }

            let (public_auth, private_auth, credentials) =
                self.refresh_chatgpt(&private.refresh_token).await?;
            self.save_chatgpt_auth(&public_auth, &private_auth)?;
            return Ok(credentials);
        }

        Ok(chatgpt_to_oauth_credentials(&auth, None))
    }

    /// Refresh the access token.
    async fn refresh(&self, refresh_token: &str) -> anyhow::Result<OAuthCredentials> {
        let resp = self
            .oauth_post(
                &self.config.token_url,
                vec![
                    ("client_id".to_string(), self.config.client_id.clone()),
                    ("refresh_token".to_string(), refresh_token.to_string()),
                    ("grant_type".to_string(), "refresh_token".to_string()),
                ],
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Token refresh failed: {}", body);
        }

        let token: TokenResponse = resp.json().await?;
        let refreshed_at = Utc::now();
        let expires_at = self.expires_at_from_token(&token, refreshed_at);
        let account_id = extract_account_id(&token);
        Ok(OAuthCredentials {
            auth_mode: Some(self.effective_auth_mode()),
            last_refresh: Some(refreshed_at),
            tokens: OAuthTokens {
                access_token: token.access_token,
                refresh_token: token
                    .refresh_token
                    .clone()
                    .or(Some(refresh_token.to_string())),
                id_token: token.id_token.clone(),
                account_id,
            },
            expires_at,
            token_type: token.token_type,
            metadata: HashMap::new(),
        })
    }

    async fn refresh_chatgpt(
        &self,
        refresh_token: &str,
    ) -> anyhow::Result<(ChatgptAuth, ChatgptPrivateAuth, OAuthCredentials)> {
        let token = self.exchange_refresh_token(refresh_token).await?;
        self.chatgpt_auth_from_token_with_fallback(token, refresh_token)
    }

    async fn exchange_refresh_token(&self, refresh_token: &str) -> anyhow::Result<TokenResponse> {
        let resp = self
            .oauth_post(
                &self.config.token_url,
                vec![
                    ("client_id".to_string(), self.config.client_id.clone()),
                    ("refresh_token".to_string(), refresh_token.to_string()),
                    ("grant_type".to_string(), "refresh_token".to_string()),
                ],
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Token refresh failed: {}", body);
        }

        Ok(resp.json().await?)
    }

    async fn exchange_browser_code_for_tokens(
        &self,
        code: &str,
        redirect_url: &str,
        code_verifier: &str,
    ) -> anyhow::Result<TokenResponse> {
        let resp = self
            .oauth_post(
                &self.config.token_url,
                vec![
                    ("grant_type".to_string(), "authorization_code".to_string()),
                    ("code".to_string(), code.to_string()),
                    ("redirect_uri".to_string(), redirect_url.to_string()),
                    ("client_id".to_string(), self.config.client_id.clone()),
                    ("code_verifier".to_string(), code_verifier.to_string()),
                ],
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Token exchange failed: {}", body);
        }

        Ok(resp.json().await?)
    }

    fn chatgpt_auth_from_token(
        &self,
        token: TokenResponse,
    ) -> anyhow::Result<(ChatgptAuth, ChatgptPrivateAuth, OAuthCredentials)> {
        self.chatgpt_auth_from_token_with_fallback(token, "")
    }

    fn chatgpt_auth_from_token_with_fallback(
        &self,
        token: TokenResponse,
        refresh_fallback: &str,
    ) -> anyhow::Result<(ChatgptAuth, ChatgptPrivateAuth, OAuthCredentials)> {
        let refreshed_at = Utc::now();
        let refresh_token = token
            .refresh_token
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| (!refresh_fallback.is_empty()).then(|| refresh_fallback.to_string()))
            .ok_or_else(|| anyhow::anyhow!("ChatGPT login did not return a refresh token"))?;
        let account_id = extract_account_id(&token).unwrap_or_default();
        let id_token = token.id_token.clone().unwrap_or_default();
        let expires_at = self.expires_at_from_token(&token, refreshed_at);

        let public_auth = ChatgptAuth {
            openai_api_key: None,
            auth_mode: self.effective_auth_mode(),
            last_refresh: refreshed_at.to_rfc3339(),
            tokens: ChatgptTokens {
                access_token: token.access_token.clone(),
                account_id: account_id.clone(),
                id_token: id_token.clone(),
            },
        };
        let private_auth = ChatgptPrivateAuth {
            refresh_token: refresh_token.clone(),
            expires_at,
            token_type: token.token_type.clone(),
        };
        let credentials = OAuthCredentials {
            auth_mode: Some(public_auth.auth_mode.clone()),
            last_refresh: Some(refreshed_at),
            tokens: OAuthTokens {
                access_token: token.access_token,
                refresh_token: Some(refresh_token),
                id_token: Some(id_token),
                account_id: Some(account_id),
            },
            expires_at,
            token_type: token.token_type,
            metadata: HashMap::new(),
        };

        Ok((public_auth, private_auth, credentials))
    }

    fn token_lifetime_secs(&self, token: &TokenResponse) -> u64 {
        token
            .expires_in
            .or(self.config.token_expires_in_fallback_secs)
            .unwrap_or(3600)
    }

    fn expires_at_from_token(
        &self,
        token: &TokenResponse,
        refreshed_at: DateTime<Utc>,
    ) -> DateTime<Utc> {
        refreshed_at
            + chrono::Duration::seconds(
                self.token_lifetime_secs(token)
                    .try_into()
                    .unwrap_or(i64::MAX),
            )
    }

    fn oauth_post(&self, url: &str, fields: Vec<(String, String)>) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", OAUTH_USER_AGENT);

        if self.config.disable_connection_reuse {
            request = request.header("Connection", "close");
        }

        match self.config.request_encoding {
            OAuthRequestEncoding::Form => request.form(&fields),
            OAuthRequestEncoding::Json => {
                let payload: serde_json::Map<String, serde_json::Value> = fields
                    .into_iter()
                    .map(|(key, value)| (key, serde_json::Value::String(value)))
                    .collect();
                request
                    .header("Content-Type", "application/json")
                    .json(&payload)
            }
        }
    }

    /// Logout: remove saved credentials for the current provider only.
    pub fn logout(&self) -> anyhow::Result<()> {
        let mut store = self.load_store()?;
        let removed = store.providers.remove(&self.provider_id).is_some();
        let removed_private = self.remove_private_credentials()?;

        if !removed && !removed_private {
            eprintln!(
                "No saved credentials found for provider '{}'.",
                self.provider_id
            );
            return Ok(());
        }

        if store.providers.is_empty() {
            if self.credentials_path.exists() {
                std::fs::remove_file(&self.credentials_path)?;
            }
        } else {
            self.save_store(&store)?;
        }

        eprintln!(
            "Credentials removed for provider '{}': {}",
            self.provider_id,
            self.credentials_path.display()
        );
        Ok(())
    }

    /// Check whether credentials exist for the current provider.
    pub fn has_credentials(&self) -> bool {
        self.load_stored_auth().is_ok()
    }

    fn save_credentials(&self, creds: &OAuthCredentials) -> anyhow::Result<()> {
        let mut store = self.load_store()?;
        store
            .providers
            .insert(self.provider_id.clone(), StoredAuth::oauth(creds.clone()));
        self.save_store(&store)
    }

    fn save_chatgpt_auth(
        &self,
        public_auth: &ChatgptAuth,
        private_auth: &ChatgptPrivateAuth,
    ) -> anyhow::Result<()> {
        let mut public_store = self.load_store()?;
        public_store.providers.insert(
            self.provider_id.clone(),
            StoredAuth::chatgpt(public_auth.clone()),
        );
        self.save_store(&public_store)?;

        let mut private_store = self.load_private_store()?;
        private_store
            .providers
            .insert(self.provider_id.clone(), private_auth.clone());
        self.save_private_store(&private_store)
    }

    #[cfg(test)]
    fn load_credentials(&self) -> anyhow::Result<OAuthCredentials> {
        match self.load_stored_auth()? {
            StoredAuth::OAuth { credentials, .. } => Ok(credentials),
            StoredAuth::Chatgpt(auth) => Ok(chatgpt_to_oauth_credentials(
                &auth,
                self.load_chatgpt_private()?.as_ref(),
            )),
            StoredAuth::ApiKey { .. } => anyhow::bail!(
                "Stored credentials for provider '{}' are API-key based, not OAuth.",
                self.provider_id
            ),
        }
    }

    fn load_stored_auth(&self) -> anyhow::Result<StoredAuth> {
        let store = self.load_store()?;
        store
            .providers
            .get(&self.provider_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No saved credentials for provider '{}'. Run 'aionrs --provider {} --login'",
                    self.provider_id,
                    self.provider_id
                )
            })
    }

    fn load_store(&self) -> anyhow::Result<AuthStore> {
        let json = match std::fs::read_to_string(&self.credentials_path) {
            Ok(json) => json,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(AuthStore::default()),
            Err(err) => return Err(err.into()),
        };

        if json.trim().is_empty() {
            return Ok(AuthStore::default());
        }

        let parsed: StoredAuthFile = serde_json::from_str(&json)?;
        Ok(match parsed {
            StoredAuthFile::Store(store) => store,
            StoredAuthFile::LegacyOAuth(credentials) => {
                let mut providers = HashMap::new();
                providers.insert(
                    LEGACY_PROVIDER_ID.to_string(),
                    StoredAuth::oauth(credentials),
                );
                AuthStore { providers }
            }
        })
    }

    fn save_store(&self, store: &AuthStore) -> anyhow::Result<()> {
        if let Some(parent) = self.credentials_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(store)?;
        std::fs::write(&self.credentials_path, json)?;
        Ok(())
    }

    fn load_chatgpt_private(&self) -> anyhow::Result<Option<ChatgptPrivateAuth>> {
        Ok(self
            .load_private_store()?
            .providers
            .get(&self.provider_id)
            .cloned())
    }

    fn load_private_store(&self) -> anyhow::Result<PrivateAuthStore> {
        let json = match std::fs::read_to_string(&self.private_credentials_path) {
            Ok(json) => json,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Ok(PrivateAuthStore::default());
            }
            Err(err) => return Err(err.into()),
        };

        if json.trim().is_empty() {
            return Ok(PrivateAuthStore::default());
        }

        Ok(serde_json::from_str(&json)?)
    }

    fn save_private_store(&self, store: &PrivateAuthStore) -> anyhow::Result<()> {
        if store.providers.is_empty() {
            if self.private_credentials_path.exists() {
                std::fs::remove_file(&self.private_credentials_path)?;
            }
            return Ok(());
        }

        if let Some(parent) = self.private_credentials_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(store)?;
        std::fs::write(&self.private_credentials_path, json)?;
        Ok(())
    }

    fn remove_private_credentials(&self) -> anyhow::Result<bool> {
        let mut store = self.load_private_store()?;
        let removed = store.providers.remove(&self.provider_id).is_some();
        self.save_private_store(&store)?;
        Ok(removed)
    }

    fn effective_auth_mode(&self) -> String {
        self.config
            .auth_mode
            .clone()
            .unwrap_or_else(|| self.provider_id.clone())
    }
}

fn chatgpt_to_oauth_credentials(
    auth: &ChatgptAuth,
    private: Option<&ChatgptPrivateAuth>,
) -> OAuthCredentials {
    OAuthCredentials {
        auth_mode: Some(auth.auth_mode.clone()),
        last_refresh: parse_last_refresh(&auth.last_refresh),
        tokens: OAuthTokens {
            access_token: auth.tokens.access_token.clone(),
            refresh_token: private.map(|value| value.refresh_token.clone()),
            id_token: Some(auth.tokens.id_token.clone()),
            account_id: Some(auth.tokens.account_id.clone()),
        },
        expires_at: private
            .map(|value| value.expires_at)
            .unwrap_or_else(far_future_expires_at),
        token_type: private
            .map(|value| value.token_type.clone())
            .unwrap_or_else(|| "Bearer".to_string()),
        metadata: HashMap::new(),
    }
}

fn build_authorize_url(
    config: &AuthConfig,
    redirect_url: &str,
    code_challenge: &str,
    state: &str,
) -> anyhow::Result<String> {
    let mut url = Url::parse(&config.auth_url)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", &config.client_id);
        pairs.append_pair("redirect_uri", redirect_url);
        pairs.append_pair(
            "scope",
            config
                .scope
                .as_deref()
                .unwrap_or("openid profile email offline_access"),
        );
        pairs.append_pair("code_challenge", code_challenge);
        pairs.append_pair("code_challenge_method", "S256");
        pairs.append_pair("state", state);
        for (key, value) in &config.authorize_params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

async fn wait_for_browser_callback(
    redirect_url: &str,
    expected_state: &str,
) -> anyhow::Result<String> {
    let redirect = Url::parse(redirect_url)?;
    let host = redirect
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("redirect_url must include a host"))?;
    if host != "localhost" && host != "127.0.0.1" {
        anyhow::bail!("redirect_url host must be localhost or 127.0.0.1");
    }
    let port = redirect
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("redirect_url must include a port"))?;
    let expected_path = redirect.path().to_string();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;

    tokio::time::timeout(
        std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS),
        async move {
            loop {
                let (mut socket, _) = listener.accept().await?;
                let mut buf = vec![0u8; 8192];
                let len = socket.read(&mut buf).await?;
                let request = String::from_utf8_lossy(&buf[..len]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let request_url = Url::parse(&format!("http://localhost{}", target))?;

                if request_url.path() != expected_path {
                    socket
                        .write_all(
                            http_html_response(
                                "404 Not Found",
                                &browser_html("Not Found", "Unexpected callback path."),
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                }

                if let Some(error) = request_url
                    .query_pairs()
                    .find_map(|(key, value)| (key == "error").then_some(value.into_owned()))
                {
                    let description = request_url
                        .query_pairs()
                        .find_map(|(key, value)| {
                            (key == "error_description").then_some(value.into_owned())
                        })
                        .unwrap_or(error.clone());
                    socket
                        .write_all(
                            http_html_response(
                                "400 Bad Request",
                                &browser_html("Authorization Failed", &description),
                            )
                            .as_bytes(),
                        )
                        .await?;
                    anyhow::bail!(description);
                }

                let state = request_url
                    .query_pairs()
                    .find_map(|(key, value)| (key == "state").then_some(value.into_owned()))
                    .unwrap_or_default();
                if state != expected_state {
                    socket
                        .write_all(
                            http_html_response(
                                "400 Bad Request",
                                &browser_html("Authorization Failed", "Invalid OAuth state."),
                            )
                            .as_bytes(),
                        )
                        .await?;
                    anyhow::bail!("Invalid OAuth state");
                }

                let code = request_url
                    .query_pairs()
                    .find_map(|(key, value)| (key == "code").then_some(value.into_owned()))
                    .ok_or_else(|| anyhow::anyhow!("Missing authorization code"))?;
                socket
                    .write_all(
                        http_html_response(
                            "200 OK",
                            &browser_html(
                                "Authorization Successful",
                                "You can close this window and return to aionrs.",
                            ),
                        )
                        .as_bytes(),
                    )
                    .await?;
                return Ok(code);
            }
        },
    )
    .await
    .map_err(|_| anyhow::anyhow!("OAuth callback timed out. Please try again."))?
}

fn browser_html(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"font-family:system-ui,-apple-system,sans-serif;display:flex;justify-content:center;\
         align-items:center;height:100vh;margin:0;background:#111827;color:#f9fafb;\">\
         <div style=\"text-align:center;max-width:28rem;padding:2rem;\">\
         <h1 style=\"margin-bottom:1rem;\">{title}</h1><p>{message}</p></div></body></html>"
    )
}

fn http_html_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn random_base64url(size: usize) -> String {
    let mut bytes = vec![0u8; size];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_pkce_pair() -> (String, String) {
    let verifier = random_base64url(32);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn extract_account_id(token: &TokenResponse) -> Option<String> {
    token
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&token.access_token))
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let claims = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(claims)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    json.get("chatgpt_account_id")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            json.get("https://api.openai.com/auth")
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            json.get("organizations")
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|value| value.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn parse_last_refresh(value: &str) -> Option<DateTime<Utc>> {
    if value.trim().is_empty() {
        return None;
    }

    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn far_future_expires_at() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::days(3650)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_manager(dir: &std::path::Path, provider_id: &str) -> OAuthManager {
        OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig::default(),
            provider_id: provider_id.to_string(),
            credentials_path: dir.join("auth.json"),
            private_credentials_path: dir.join("auth-private.json"),
        }
    }

    fn make_credentials(hours_from_now: i64) -> OAuthCredentials {
        OAuthCredentials {
            auth_mode: Some("chatgpt".to_string()),
            last_refresh: Some(Utc::now()),
            tokens: OAuthTokens {
                access_token: "test-access-token".to_string(),
                refresh_token: Some("test-refresh-token".to_string()),
                id_token: Some("test-id-token".to_string()),
                account_id: None,
            },
            expires_at: Utc::now() + chrono::Duration::hours(hours_from_now),
            token_type: "Bearer".to_string(),
            metadata: HashMap::new(),
        }
    }

    fn jwt_with_account(account_id: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "chatgpt_account_id": account_id,
            })
            .to_string(),
        );
        format!("{header}.{claims}.signature")
    }

    #[test]
    fn test_copilot_auth_defaults_include_device_flow_and_api_headers() {
        let config = AuthConfig::for_provider("copilot").unwrap();

        assert_eq!(config.flow, AuthFlow::DeviceCode);
        assert_eq!(config.scope.as_deref(), Some("read:user"));
        assert_eq!(
            config.api_base_url.as_deref(),
            Some("https://api.githubcopilot.com")
        );
        assert_eq!(config.api_path.as_deref(), Some("/chat/completions"));
        assert_eq!(config.model_discovery_path.as_deref(), Some("/models"));
        assert_eq!(
            config.api_headers.get("Openai-Intent").map(String::as_str),
            Some("conversation-edits")
        );
        assert_eq!(
            config.api_headers.get("x-initiator").map(String::as_str),
            Some("agent")
        );
        assert_eq!(config.request_encoding, OAuthRequestEncoding::Json);
        assert!(config.http1_only);
        assert!(config.disable_connection_reuse);
    }

    #[test]
    fn test_chatgpt_auth_defaults_include_codex_headers() {
        let config = AuthConfig::for_provider("chatgpt").unwrap();

        assert_eq!(config.flow, AuthFlow::BrowserPkce);
        assert_eq!(config.api_base_url.as_deref(), Some("https://chatgpt.com"));
        assert_eq!(
            config.api_path.as_deref(),
            Some("/backend-api/codex/responses")
        );
        assert_eq!(
            config.model_discovery_path.as_deref(),
            Some("/backend-api/codex/models")
        );
        assert_eq!(
            config.usage_path.as_deref(),
            Some("/backend-api/wham/usage")
        );
        assert_eq!(
            config.api_headers.get("originator").map(String::as_str),
            Some("aionrs")
        );
        assert_eq!(
            config.api_headers.get("User-Agent").map(String::as_str),
            Some(OAUTH_USER_AGENT)
        );
        assert_eq!(
            config
                .api_headers
                .get("Accept-Encoding")
                .map(String::as_str),
            Some("identity")
        );
        assert!(config.use_responses_api);
        assert!(config.system_as_instructions);
        assert!(config.http1_only);
        assert!(config.disable_connection_reuse);
    }

    #[tokio::test]
    async fn test_save_and_load_credentials() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path(), "anthropic");
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();
        let loaded = manager.load_credentials().unwrap();

        assert_eq!(loaded.tokens.access_token, "test-access-token");
        assert_eq!(
            loaded.tokens.refresh_token,
            Some("test-refresh-token".to_string())
        );
        assert_eq!(loaded.tokens.id_token, Some("test-id-token".to_string()));
        assert_eq!(loaded.token_type, "Bearer");
        let diff = (loaded.expires_at - creds.expires_at).num_seconds().abs();
        assert!(diff <= 1, "expires_at mismatch: diff={diff}s");
    }

    #[tokio::test]
    async fn test_has_credentials_false_when_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path(), "anthropic");

        assert!(!manager.has_credentials());
    }

    #[tokio::test]
    async fn test_logout_deletes_only_selected_provider() {
        let tmp = TempDir::new().unwrap();
        let anthropic = test_manager(tmp.path(), "anthropic");
        let openai = test_manager(tmp.path(), "openai");
        let creds = make_credentials(1);

        anthropic.save_credentials(&creds).unwrap();
        openai
            .save_credentials(&OAuthCredentials {
                tokens: OAuthTokens {
                    access_token: "openai-token".to_string(),
                    ..make_credentials(1).tokens
                },
                ..make_credentials(1)
            })
            .unwrap();

        anthropic.logout().unwrap();

        assert!(!anthropic.has_credentials());
        assert!(openai.has_credentials());
        assert!(openai.credentials_path.exists());
    }

    #[tokio::test]
    async fn test_provider_scoped_storage_is_isolated() {
        let tmp = TempDir::new().unwrap();
        let anthropic = test_manager(tmp.path(), "anthropic");
        let openai = test_manager(tmp.path(), "openai");

        anthropic.save_credentials(&make_credentials(1)).unwrap();
        assert!(anthropic.has_credentials());
        assert!(!openai.has_credentials());

        openai
            .save_credentials(&OAuthCredentials {
                tokens: OAuthTokens {
                    access_token: "openai-token".to_string(),
                    ..make_credentials(1).tokens
                },
                ..make_credentials(1)
            })
            .unwrap();

        assert_eq!(
            anthropic.load_credentials().unwrap().tokens.access_token,
            "test-access-token"
        );
        assert_eq!(
            openai.load_credentials().unwrap().tokens.access_token,
            "openai-token"
        );
    }

    #[tokio::test]
    async fn test_load_legacy_credentials_for_anthropic() {
        let tmp = TempDir::new().unwrap();
        let legacy = make_credentials(1);
        std::fs::write(
            tmp.path().join("auth.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let anthropic = test_manager(tmp.path(), "anthropic");
        let openai = test_manager(tmp.path(), "openai");

        assert_eq!(
            anthropic.load_credentials().unwrap().tokens.access_token,
            "test-access-token"
        );
        assert!(!openai.has_credentials());
    }

    #[tokio::test]
    async fn test_get_token_returns_valid_token() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path(), "anthropic");
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "test-access-token");
    }

    #[tokio::test]
    async fn test_get_token_refreshes_expired() {
        let tmp = TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        let manager = OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig {
                auth_url: mock_server.uri(),
                token_url: format!("{}/token", mock_server.uri()),
                client_id: "test".to_string(),
                auth_mode: Some("chatgpt".to_string()),
                ..AuthConfig::default()
            },
            provider_id: "anthropic".to_string(),
            credentials_path: tmp.path().join("auth.json"),
            private_credentials_path: tmp.path().join("auth-private.json"),
        };

        let expired_creds = make_credentials(-1);
        manager.save_credentials(&expired_creds).unwrap();

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-token",
                "refresh_token": "new-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&mock_server)
            .await;

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "new-token");

        let reloaded = manager.load_credentials().unwrap();
        assert_eq!(reloaded.tokens.access_token, "new-token");
        assert_eq!(
            reloaded.tokens.refresh_token,
            Some("new-refresh".to_string())
        );
    }

    #[tokio::test]
    async fn test_chatgpt_auth_shape_is_loaded_verbatim() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path(), "openai");
        let auth = ChatgptAuth {
            openai_api_key: None,
            auth_mode: "chatgpt".to_string(),
            last_refresh: String::new(),
            tokens: ChatgptTokens {
                access_token: "chatgpt-access".to_string(),
                account_id: "acct-123".to_string(),
                id_token: "id-123".to_string(),
            },
        };

        let mut store = AuthStore::default();
        store
            .providers
            .insert("openai".to_string(), StoredAuth::chatgpt(auth.clone()));
        manager.save_store(&store).unwrap();

        let json = std::fs::read_to_string(tmp.path().join("auth.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed["providers"]["openai"],
            serde_json::json!({
                "OPENAI_API_KEY": null,
                "auth_mode": "chatgpt",
                "last_refresh": "",
                "tokens": {
                    "access_token": "chatgpt-access",
                    "account_id": "acct-123",
                    "id_token": "id-123"
                }
            })
        );

        let loaded = manager.load_credentials().unwrap();
        assert_eq!(loaded.tokens.access_token, "chatgpt-access");
        assert_eq!(loaded.tokens.id_token, Some("id-123".to_string()));
        assert_eq!(loaded.tokens.account_id, Some("acct-123".to_string()));
        assert_eq!(loaded.auth_mode.as_deref(), Some("chatgpt"));
    }

    #[tokio::test]
    async fn test_chatgpt_refresh_uses_private_sidecar() {
        let tmp = TempDir::new().unwrap();
        let mock_server = MockServer::start().await;
        let manager = OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig {
                auth_url: format!("{}/oauth/authorize", mock_server.uri()),
                token_url: format!("{}/oauth/token", mock_server.uri()),
                client_id: "test-client".to_string(),
                auth_mode: Some("chatgpt".to_string()),
                flow: AuthFlow::BrowserPkce,
                redirect_url: Some(CHATGPT_REDIRECT_URL.to_string()),
                scope: Some("openid profile email offline_access".to_string()),
                authorize_params: HashMap::new(),
                api_base_url: Some("https://chatgpt.com".to_string()),
                api_path: Some("/backend-api/codex/responses".to_string()),
                model_discovery_path: None,
                usage_path: None,
                api_headers: HashMap::new(),
                use_responses_api: true,
                system_as_instructions: true,
                account_id_header: Some("ChatGPT-Account-Id".to_string()),
                token_expires_in_fallback_secs: None,
                request_encoding: OAuthRequestEncoding::Form,
                http1_only: false,
                disable_connection_reuse: false,
            },
            provider_id: "chatgpt".to_string(),
            credentials_path: tmp.path().join("auth.json"),
            private_credentials_path: tmp.path().join("auth-private.json"),
        };

        manager
            .save_chatgpt_auth(
                &ChatgptAuth {
                    openai_api_key: None,
                    auth_mode: "chatgpt".to_string(),
                    last_refresh: String::new(),
                    tokens: ChatgptTokens {
                        access_token: "stale-access".to_string(),
                        account_id: "acct-old".to_string(),
                        id_token: "stale-id".to_string(),
                    },
                },
                &ChatgptPrivateAuth {
                    refresh_token: "refresh-1".to_string(),
                    expires_at: Utc::now() - chrono::Duration::minutes(5),
                    token_type: "Bearer".to_string(),
                },
            )
            .unwrap();

        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-access",
                "refresh_token": "refresh-2",
                "id_token": jwt_with_account("acct-new"),
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&mock_server)
            .await;

        let creds = manager.get_credentials().await.unwrap();
        assert_eq!(creds.tokens.access_token, "fresh-access");
        assert_eq!(creds.tokens.account_id.as_deref(), Some("acct-new"));
        assert_eq!(creds.tokens.refresh_token.as_deref(), Some("refresh-2"));

        let public_json = std::fs::read_to_string(tmp.path().join("auth.json")).unwrap();
        let public_store: serde_json::Value = serde_json::from_str(&public_json).unwrap();
        assert_eq!(
            public_store["providers"]["chatgpt"]["tokens"]["access_token"],
            "fresh-access"
        );
        assert_eq!(
            public_store["providers"]["chatgpt"]["tokens"]["account_id"],
            "acct-new"
        );

        let private_json = std::fs::read_to_string(tmp.path().join("auth-private.json")).unwrap();
        let private_store: serde_json::Value = serde_json::from_str(&private_json).unwrap();
        assert_eq!(
            private_store["providers"]["chatgpt"]["refresh_token"],
            "refresh-2"
        );
    }
}
