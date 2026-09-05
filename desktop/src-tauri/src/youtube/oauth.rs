use super::{
    config::{OAuthClientConfig, SecretString},
    models::AccountSummary,
    vault::TokenVault,
};
use crate::AppError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{SecondsFormat, Utc};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};
use url::Url;

pub const YOUTUBE_UPLOAD_SCOPE: &str = "https://www.googleapis.com/auth/youtube.upload";
const CHANNELS_URL: &str = "https://www.googleapis.com/youtube/v3/channels";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const CALLBACK_LIMIT: usize = 8 * 1024;

pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), AppError> + Send + Sync>;

pub fn system_browser_opener() -> BrowserOpener {
    Arc::new(|url| {
        tauri_plugin_opener::open_url(url, None::<&str>).map_err(|_| {
            AppError::new(
                "OAUTH_BROWSER_OPEN_FAILED",
                "无法打开系统浏览器完成 YouTube 授权",
            )
        })
    })
}

struct AuthorizationRequest {
    url: Url,
    redirect_uri: String,
}

fn build_authorization_request(
    config: &OAuthClientConfig,
    port: u16,
    state: &str,
    verifier: &str,
) -> Result<AuthorizationRequest, AppError> {
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback");
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&config.auth_uri)
        .map_err(|_| AppError::new("OAUTH_AUTH_URI_INVALID", "OAuth 授权地址无效"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", YOUTUBE_UPLOAD_SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    Ok(AuthorizationRequest { url, redirect_uri })
}

fn parse_callback_target(target: &str, expected_state: &str) -> Result<String, AppError> {
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| AppError::new("OAUTH_CALLBACK_INVALID", "OAuth 回调无效"))?;
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| AppError::new("OAUTH_STATE_MISMATCH", "OAuth 授权状态校验失败"))?;
    if state
        .as_bytes()
        .ct_eq(expected_state.as_bytes())
        .unwrap_u8()
        != 1
    {
        return Err(AppError::new(
            "OAUTH_STATE_MISMATCH",
            "OAuth 授权状态校验失败",
        ));
    }
    if let Some((_, error)) = url.query_pairs().find(|(key, _)| key == "error") {
        return Err(AppError::new(
            if error == "access_denied" {
                "OAUTH_DENIED"
            } else {
                "OAUTH_CALLBACK_FAILED"
            },
            "YouTube 授权未完成",
        ));
    }
    url.query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or_else(|| AppError::new("OAUTH_CALLBACK_INVALID", "OAuth 回调缺少授权码"))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct ChannelList {
    items: Vec<ChannelResource>,
}

#[derive(Deserialize)]
struct ChannelResource {
    id: String,
    snippet: ChannelSnippet,
}

#[derive(Deserialize)]
struct ChannelSnippet {
    title: String,
}

pub struct OAuthService {
    config: OAuthClientConfig,
    client: Client,
    vault: Arc<dyn TokenVault>,
    opener: BrowserOpener,
}

impl OAuthService {
    pub fn new(config: OAuthClientConfig, vault: Arc<dyn TokenVault>) -> Self {
        Self {
            config,
            client: Client::new(),
            vault,
            opener: system_browser_opener(),
        }
    }

    #[cfg(test)]
    pub fn with_opener(mut self, opener: BrowserOpener) -> Self {
        self.opener = opener;
        self
    }

    pub async fn authorize(&self) -> Result<AccountSummary, AppError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|error| {
            AppError::with_cause(
                "OAUTH_CALLBACK_BIND_FAILED",
                "无法启动 OAuth 本机回调",
                error.to_string(),
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                AppError::with_cause(
                    "OAUTH_CALLBACK_BIND_FAILED",
                    "无法读取 OAuth 本机回调",
                    error.to_string(),
                )
            })?
            .port();
        let state = random_secret()?;
        let verifier = random_secret()?;
        let request = build_authorization_request(&self.config, port, &state, &verifier)?;
        (self.opener)(request.url.as_str())?;
        let (mut stream, _) = timeout(Duration::from_secs(180), listener.accept())
            .await
            .map_err(|_| AppError::new("OAUTH_CALLBACK_TIMEOUT", "YouTube 授权回调超时"))?
            .map_err(|error| {
                AppError::with_cause("OAUTH_CALLBACK_FAILED", "OAuth 回调失败", error.to_string())
            })?;
        let mut buffer = vec![0u8; CALLBACK_LIMIT];
        let size = stream.read(&mut buffer).await.map_err(|error| {
            AppError::with_cause(
                "OAUTH_CALLBACK_FAILED",
                "OAuth 回调读取失败",
                error.to_string(),
            )
        })?;
        let request_line = std::str::from_utf8(&buffer[..size])
            .ok()
            .and_then(|value| value.lines().next())
            .ok_or_else(|| AppError::new("OAUTH_CALLBACK_INVALID", "OAuth 回调无效"))?;
        let target = request_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| AppError::new("OAUTH_CALLBACK_INVALID", "OAuth 回调无效"))?;
        let code = parse_callback_target(target, &state);
        let response = if code.is_ok() {
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 38\r\nConnection: close\r\n\r\nAuthorization complete. Return to app."
        } else {
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 21\r\nConnection: close\r\n\r\nAuthorization failed."
        };
        let _ = stream.write_all(response.as_bytes()).await;
        let code = code?;
        let token = self
            .exchange_code(&code, &verifier, &request.redirect_uri)
            .await?;
        let refresh = token.refresh_token.ok_or_else(|| {
            AppError::new(
                "OAUTH_REFRESH_TOKEN_MISSING",
                "Google 未返回可持久授权，请重试",
            )
        })?;
        let channel = self.channel(&token.access_token).await?;
        self.vault
            .save_refresh_token(&channel.channel_id, &refresh)?;
        Ok(channel)
    }

    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenResponse, AppError> {
        let mut fields = vec![
            ("client_id", self.config.client_id.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ];
        if let Some(secret) = &self.config.client_secret {
            fields.push(("client_secret", secret.expose_secret()));
        }
        let response = self
            .client
            .post(&self.config.token_uri)
            .form(&fields)
            .send()
            .await
            .map_err(|_| AppError::new("OAUTH_TOKEN_EXCHANGE_FAILED", "OAuth 令牌交换失败"))?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "OAUTH_TOKEN_EXCHANGE_FAILED",
                "OAuth 令牌交换失败",
            ));
        }
        response
            .json()
            .await
            .map_err(|_| AppError::new("OAUTH_TOKEN_EXCHANGE_FAILED", "OAuth 令牌响应无效"))
    }

    async fn channel(&self, access_token: &str) -> Result<AccountSummary, AppError> {
        let response = self
            .client
            .get(CHANNELS_URL)
            .query(&[("part", "snippet"), ("mine", "true")])
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| AppError::new("YOUTUBE_CHANNEL_LOOKUP_FAILED", "无法读取 YouTube 频道"))?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "YOUTUBE_CHANNEL_LOOKUP_FAILED",
                "无法读取 YouTube 频道",
            ));
        }
        let channel = response
            .json::<ChannelList>()
            .await
            .ok()
            .and_then(|value| value.items.into_iter().next())
            .ok_or_else(|| {
                AppError::new("YOUTUBE_CHANNEL_NOT_FOUND", "授权账号下没有 YouTube 频道")
            })?;
        Ok(AccountSummary {
            channel_id: channel.id,
            title: channel.snippet.title,
            authorized_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        })
    }

    pub async fn access_token(&self, channel_id: &str) -> Result<SecretString, AppError> {
        let refresh = self.vault.load_refresh_token(channel_id)?;
        let mut fields = vec![
            ("client_id", self.config.client_id.as_str()),
            ("refresh_token", refresh.expose_secret()),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = &self.config.client_secret {
            fields.push(("client_secret", secret.expose_secret()));
        }
        let response = self
            .client
            .post(&self.config.token_uri)
            .form(&fields)
            .send()
            .await
            .map_err(|_| AppError::new("OAUTH_REFRESH_FAILED", "YouTube 授权刷新失败"))?;
        if !response.status().is_success() {
            return Err(AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道"));
        }
        let token = response
            .json::<TokenResponse>()
            .await
            .map_err(|_| AppError::new("OAUTH_REFRESH_FAILED", "YouTube 授权刷新失败"))?;
        Ok(SecretString::new(token.access_token))
    }

    pub async fn revoke(&self, channel_id: &str) -> Result<(), AppError> {
        let refresh = self.vault.load_refresh_token(channel_id)?;
        let response = self
            .client
            .post(REVOKE_URL)
            .form(&[("token", refresh.expose_secret())])
            .send()
            .await
            .map_err(|_| AppError::new("OAUTH_REVOKE_FAILED", "YouTube 授权撤销失败"))?;
        if !response.status().is_success() {
            return Err(AppError::new("OAUTH_REVOKE_FAILED", "YouTube 授权撤销失败"));
        }
        self.vault.delete_refresh_token(channel_id)
    }
}

fn random_secret() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| AppError::new("OAUTH_RANDOM_FAILED", "无法创建安全 OAuth 请求"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_uses_pkce_state_loopback_and_only_upload_scope() {
        let config = OAuthClientConfig::from_json(r#"{"installed":{"client_id":"synthetic.apps.googleusercontent.com","client_secret":"secret","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://127.0.0.1"]}}"#).unwrap();
        let request =
            build_authorization_request(&config, 49152, "state-value", "verifier-value").unwrap();
        let query = request
            .url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("scope").map(|value| value.as_ref()),
            Some(YOUTUBE_UPLOAD_SCOPE)
        );
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some("state-value")
        );
        assert_eq!(
            query
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert_eq!(
            query.get("redirect_uri").map(|value| value.as_ref()),
            Some("http://127.0.0.1:49152/oauth/callback")
        );
        assert!(!request.url.as_str().contains("secret"));
    }

    #[test]
    fn callback_state_mismatch_is_rejected_without_exposing_code() {
        let error = parse_callback_target(
            "/oauth/callback?state=wrong&code=synthetic-secret-code",
            "expected",
        )
        .unwrap_err();
        assert_eq!(error.code, "OAUTH_STATE_MISMATCH");
        assert!(!error.message.contains("synthetic-secret-code"));
    }
}
