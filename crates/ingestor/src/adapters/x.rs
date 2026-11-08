//! X (Twitter) liked posts through the official API v2.
//!
//! Likes are private, so this needs a user-context token: OAuth 2.0
//! Authorization Code with PKCE, scopes `tweet.read users.read like.read
//! offline.access`. `login_*` runs that flow once; the resulting tokens are
//! kept in a JSON file and refreshed automatically (X refresh tokens are
//! single-use, so every refresh rewrites the file).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::Bytes;
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{LikedPage, SourceAdapter, SourcePost};

const API_BASE: &str = "https://api.x.com/2";
const AUTHORIZE_URL: &str = "https://x.com/i/oauth2/authorize";
const TOKEN_URL: &str = "https://api.x.com/2/oauth2/token";
const SCOPES: &str = "tweet.read users.read like.read offline.access";
const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:8765/callback";
/// Refresh this long before the access token actually expires.
const EXPIRY_MARGIN_SECS: u64 = 60;
/// Longest we'll sleep for a rate-limit window before giving up.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(16 * 60);

/// App credentials from the X developer portal (User authentication settings).
#[derive(Debug, Clone)]
pub struct XAppConfig {
    pub client_id: String,
    /// Only for "confidential" apps; public (native) apps have none.
    pub client_secret: Option<String>,
    /// Must exactly match a callback URL registered on the app.
    pub redirect_uri: String,
    pub token_path: PathBuf,
}

impl XAppConfig {
    /// `FULGORART_X_CLIENT_ID` (required), `FULGORART_X_CLIENT_SECRET`,
    /// `FULGORART_X_REDIRECT_URI`, `FULGORART_X_TOKEN_PATH` (else `default_token_path`).
    pub fn from_env(default_token_path: PathBuf) -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Ok(Self {
            client_id: var("FULGORART_X_CLIENT_ID").context(
                "FULGORART_X_CLIENT_ID must be set (OAuth 2.0 client ID of your X app)",
            )?,
            client_secret: var("FULGORART_X_CLIENT_SECRET"),
            redirect_uri: var("FULGORART_X_REDIRECT_URI")
                .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_owned()),
            token_path: var("FULGORART_X_TOKEN_PATH")
                .map(PathBuf::from)
                .unwrap_or(default_token_path),
        })
    }
}

/// Tokens saved to `XAppConfig::token_path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: u64,
    pub user_id: String,
    pub username: String,
}

impl XTokens {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "No X login found at '{}'. Run `cli ingestor x login` first",
                path.display()
            )
        })?;
        serde_json::from_str(&text).with_context(|| format!("Corrupt token file '{}'", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        // Write then rename so a crash can't leave a half-written file and lose
        // the (single-use) refresh token.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
            .with_context(|| format!("Failed to write token file '{}'", path.display()))
    }

    fn expired(&self) -> bool {
        now_secs() + EXPIRY_MARGIN_SECS >= self.expires_at
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_urlsafe(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow::anyhow!("No system randomness available: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// PKCE S256 challenge for a verifier (RFC 7636).
fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// First half of the login: the URL to open, plus what `login_finish` needs.
pub struct LoginStart {
    pub authorize_url: String,
    state: String,
    verifier: String,
}

pub fn login_start(config: &XAppConfig) -> Result<LoginStart> {
    let state = random_urlsafe(16)?;
    let verifier = random_urlsafe(32)?;
    let authorize_url = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("response_type", "code"),
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("scope", SCOPES),
            ("state", state.as_str()),
            ("code_challenge", pkce_challenge(&verifier).as_str()),
            ("code_challenge_method", "S256"),
        ],
    )?
    .to_string()
    // form encoding writes spaces as '+'; X's docs use %20 in `scope`. A
    // literal '+' in any value is already escaped as %2B, so this is safe.
    .replace('+', "%20");
    Ok(LoginStart {
        authorize_url,
        state,
        verifier,
    })
}

/// Pull `code` out of the URL the browser was redirected to, checking `state`.
fn code_from_redirect(redirected: &str, expected_state: &str) -> Result<String> {
    let url = url::Url::parse(redirected.trim())
        .context("Paste the whole URL from the browser's address bar")?;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    if let Some(error) = params.get("error") {
        bail!("X refused the authorization: {error}");
    }
    if params.get("state").map(String::as_str) != Some(expected_state) {
        bail!("State mismatch: this URL is not from the login link just printed");
    }
    params
        .get("code")
        .cloned()
        .context("No `code` in the URL")
}

/// Second half: exchange the redirect for tokens, look up the user, save.
pub async fn login_finish(
    config: &XAppConfig,
    start: &LoginStart,
    redirected_url: &str,
) -> Result<XTokens> {
    let code = code_from_redirect(redirected_url, &start.state)?;
    let client = reqwest::Client::new();
    let response = token_request(
        &client,
        config,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("code_verifier", start.verifier.as_str()),
        ],
    )
    .await?;

    #[derive(Deserialize)]
    struct Me {
        data: MeData,
    }
    #[derive(Deserialize)]
    struct MeData {
        id: String,
        username: String,
    }
    let me: Me = client
        .get(format!("{API_BASE}/users/me"))
        .bearer_auth(&response.access_token)
        .send()
        .await?
        .error_for_status()
        .context("Failed to look up the logged-in X user")?
        .json()
        .await?;

    let tokens = XTokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: now_secs() + response.expires_in.unwrap_or(7200),
        user_id: me.data.id,
        username: me.data.username,
    };
    tokens.save(&config.token_path)?;
    Ok(tokens)
}

async fn token_request(
    client: &reqwest::Client,
    config: &XAppConfig,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let mut form = form.to_vec();
    let mut request = client.post(TOKEN_URL);
    match &config.client_secret {
        Some(secret) => request = request.basic_auth(&config.client_id, Some(secret)),
        None => form.push(("client_id", config.client_id.as_str())),
    }
    let response = request.form(&form).send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("X token endpoint returned {status}: {body}");
    }
    Ok(response.json().await?)
}

/// Liked posts of the logged-in user.
pub struct XAdapter {
    config: XAppConfig,
    client: reqwest::Client,
    tokens: Mutex<XTokens>,
}

impl XAdapter {
    pub fn new(config: XAppConfig) -> Result<Self> {
        let tokens = XTokens::load(&config.token_path)?;
        Ok(Self {
            config,
            client: reqwest::Client::new(),
            tokens: Mutex::new(tokens),
        })
    }

    pub async fn username(&self) -> String {
        self.tokens.lock().await.username.clone()
    }

    /// Current access token, refreshing first if it is (about to be) expired.
    async fn access_token(&self, force_refresh: bool) -> Result<String> {
        let mut tokens = self.tokens.lock().await;
        if force_refresh || tokens.expired() {
            let refresh_token = tokens.refresh_token.clone().context(
                "X access token expired and there is no refresh token; run `cli ingestor x login`",
            )?;
            let response = token_request(
                &self.client,
                &self.config,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                ],
            )
            .await
            .context("Failed to refresh the X token; run `cli ingestor x login` again")?;
            tokens.access_token = response.access_token;
            tokens.refresh_token = response.refresh_token.or(Some(refresh_token));
            tokens.expires_at = now_secs() + response.expires_in.unwrap_or(7200);
            tokens.save(&self.config.token_path)?;
            tracing::debug!("refreshed X access token");
        }
        Ok(tokens.access_token.clone())
    }

    /// GET an API URL as the user: refreshes once on 401, waits out 429s.
    async fn get_api<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let mut refreshed = false;
        loop {
            let token = self.access_token(false).await?;
            let response = self
                .client
                .get(url)
                .query(query)
                .bearer_auth(token)
                .send()
                .await?;

            match response.status() {
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    self.access_token(true).await?;
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = rate_limit_wait(response.headers());
                    if wait > MAX_RATE_LIMIT_WAIT {
                        bail!("X rate limit hit; resets in {}s", wait.as_secs());
                    }
                    tracing::warn!(wait_secs = wait.as_secs(), "X rate limit hit, waiting");
                    println!("rate_limited waiting={}s", wait.as_secs());
                    tokio::time::sleep(wait).await;
                }
                status if !status.is_success() => {
                    let body = response.text().await.unwrap_or_default();
                    bail!("X API {url} returned {status}: {body}");
                }
                _ => return Ok(response.json().await?),
            }
        }
    }
}

fn rate_limit_wait(headers: &header::HeaderMap) -> Duration {
    let reset = headers
        .get("x-rate-limit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    match reset {
        Some(reset) => Duration::from_secs(reset.saturating_sub(now_secs()) + 1),
        None => Duration::from_secs(60),
    }
}

// ---- liked_tweets response ----

#[derive(Debug, Deserialize)]
struct LikedTweetsResponse {
    #[serde(default)]
    data: Vec<serde_json::Value>,
    #[serde(default)]
    includes: Includes,
    #[serde(default)]
    meta: Meta,
}

#[derive(Debug, Default, Deserialize)]
struct Includes {
    #[serde(default)]
    media: Vec<Media>,
    #[serde(default)]
    users: Vec<User>,
}

#[derive(Debug, Deserialize)]
struct Media {
    media_key: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct User {
    id: String,
    name: Option<String>,
    username: String,
}

#[derive(Debug, Default, Deserialize)]
struct Meta {
    next_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Tweet {
    id: String,
    text: Option<String>,
    created_at: Option<String>,
    author_id: Option<String>,
    #[serde(default)]
    attachments: Attachments,
}

#[derive(Debug, Default, Deserialize)]
struct Attachments {
    #[serde(default)]
    media_keys: Vec<String>,
}

/// Photo URL at original resolution (`pbs.twimg.com/media/X.jpg?name=orig`).
fn original_photo_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let others: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| k != "name")
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            parsed
                .query_pairs_mut()
                .clear()
                .extend_pairs(others)
                .append_pair("name", "orig");
            parsed.to_string()
        }
        Err(_) => url.to_owned(),
    }
}

fn parse_liked_page(response: LikedTweetsResponse) -> Result<LikedPage> {
    let media: HashMap<&str, &Media> = response
        .includes
        .media
        .iter()
        .map(|m| (m.media_key.as_str(), m))
        .collect();
    let users: HashMap<&str, &User> = response
        .includes
        .users
        .iter()
        .map(|u| (u.id.as_str(), u))
        .collect();

    let mut posts = Vec::with_capacity(response.data.len());
    for raw in &response.data {
        let tweet: Tweet = serde_json::from_value(raw.clone()).context("Unexpected post shape")?;
        let author = tweet.author_id.as_deref().and_then(|id| users.get(id));
        // Photos only; videos and GIFs have no `url`, only a preview image.
        let image_urls = tweet
            .attachments
            .media_keys
            .iter()
            .filter_map(|key| media.get(key.as_str()))
            .filter(|m| m.kind == "photo")
            .filter_map(|m| m.url.as_deref())
            .map(original_photo_url)
            .collect();
        let username = author.map(|a| a.username.as_str()).unwrap_or("i");

        posts.push(SourcePost {
            source_type: X_SOURCE_TYPE.to_owned(),
            source_post_url: format!("https://x.com/{username}/status/{}", tweet.id),
            source_post_id: tweet.id,
            liked_at: None, // the API doesn't say when a like happened
            posted_at: tweet.created_at,
            caption: tweet.text,
            author_name: author.and_then(|a| a.name.clone()),
            author_source_id: tweet.author_id,
            author_url: author.map(|a| format!("https://x.com/{}", a.username)),
            image_urls,
            raw_json: Some(raw.to_string()),
        });
    }

    Ok(LikedPage {
        posts,
        next_cursor: response.meta.next_token,
    })
}

pub const X_SOURCE_TYPE: &str = "x";

#[async_trait]
impl SourceAdapter for XAdapter {
    fn source_type(&self) -> &str {
        X_SOURCE_TYPE
    }

    async fn fetch_liked_page(&self, cursor: Option<&str>) -> Result<LikedPage> {
        let user_id = self.tokens.lock().await.user_id.clone();
        let mut query = vec![
            ("max_results", "100"),
            ("expansions", "attachments.media_keys,author_id"),
            ("tweet.fields", "created_at,author_id,attachments,text"),
            ("media.fields", "type,url,width,height"),
            ("user.fields", "username,name"),
        ];
        if let Some(cursor) = cursor {
            query.push(("pagination_token", cursor));
        }
        let response: LikedTweetsResponse = self
            .get_api(&format!("{API_BASE}/users/{user_id}/liked_tweets"), &query)
            .await?;
        parse_liked_page(response)
    }

    async fn download_image(&self, url: &str) -> Result<(Bytes, String)> {
        // pbs.twimg.com is public; don't send the user token there.
        let response = self.client.get(url).send().await?.error_for_status()?;
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_owned();
        Ok((response.bytes().await?, content_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn login_url_has_pkce_and_scopes() {
        let config = XAppConfig {
            client_id: "cid".into(),
            client_secret: None,
            redirect_uri: DEFAULT_REDIRECT_URI.into(),
            token_path: "unused".into(),
        };
        let start = login_start(&config).unwrap();
        let url = url::Url::parse(&start.authorize_url).unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["scope"], SCOPES);
        assert_eq!(params["state"], start.state);
        assert_eq!(params["code_challenge"], pkce_challenge(&start.verifier));
        assert!(start.authorize_url.contains("scope=tweet.read%20users.read"));
    }

    #[test]
    fn code_from_redirect_checks_state() {
        let url = "http://127.0.0.1:8765/callback?state=abc&code=xyz";
        assert_eq!(code_from_redirect(url, "abc").unwrap(), "xyz");
        assert!(code_from_redirect(url, "other").is_err());
        assert!(code_from_redirect("http://127.0.0.1:8765/callback?error=access_denied&state=abc", "abc").is_err());
        assert!(code_from_redirect("xyz", "abc").is_err());
    }

    #[test]
    fn original_photo_url_sets_name_orig() {
        assert_eq!(
            original_photo_url("https://pbs.twimg.com/media/AbC.jpg"),
            "https://pbs.twimg.com/media/AbC.jpg?name=orig"
        );
        assert_eq!(
            original_photo_url("https://pbs.twimg.com/media/AbC?format=png&name=small"),
            "https://pbs.twimg.com/media/AbC?format=png&name=orig"
        );
    }

    #[test]
    fn parses_liked_tweets_page() {
        let body = r#"{
          "data": [
            {"id": "1", "text": "two pics", "created_at": "2026-11-01T00:00:00.000Z",
             "author_id": "u1", "attachments": {"media_keys": ["3_a", "3_b"]}},
            {"id": "2", "text": "a video", "author_id": "u2",
             "attachments": {"media_keys": ["7_v"]}},
            {"id": "3", "text": "text only", "author_id": "u1"}
          ],
          "includes": {
            "media": [
              {"media_key": "3_a", "type": "photo", "url": "https://pbs.twimg.com/media/A.jpg"},
              {"media_key": "3_b", "type": "photo", "url": "https://pbs.twimg.com/media/B.png"},
              {"media_key": "7_v", "type": "video", "preview_image_url": "https://pbs.twimg.com/v.jpg"}
            ],
            "users": [{"id": "u1", "name": "Artist", "username": "artist"}]
          },
          "meta": {"result_count": 3, "next_token": "NEXT"}
        }"#;
        let page = parse_liked_page(serde_json::from_str(body).unwrap()).unwrap();

        assert_eq!(page.next_cursor.as_deref(), Some("NEXT"));
        assert_eq!(page.posts.len(), 3);
        let first = &page.posts[0];
        assert_eq!(first.source_type, "x");
        assert_eq!(first.source_post_url, "https://x.com/artist/status/1");
        assert_eq!(first.author_url.as_deref(), Some("https://x.com/artist"));
        assert_eq!(first.author_name.as_deref(), Some("Artist"));
        assert_eq!(first.posted_at.as_deref(), Some("2026-11-01T00:00:00.000Z"));
        assert_eq!(
            first.image_urls,
            [
                "https://pbs.twimg.com/media/A.jpg?name=orig",
                "https://pbs.twimg.com/media/B.png?name=orig"
            ]
        );
        assert!(page.posts[1].image_urls.is_empty(), "videos are skipped");
        // author missing from includes still gives a working URL
        assert_eq!(page.posts[1].source_post_url, "https://x.com/i/status/2");
        assert!(page.posts[2].image_urls.is_empty());
    }

    #[test]
    fn last_page_has_no_cursor() {
        let page = parse_liked_page(serde_json::from_str(r#"{"meta": {"result_count": 0}}"#).unwrap()).unwrap();
        assert!(page.posts.is_empty());
        assert!(page.next_cursor.is_none());
    }
}
