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
pub const YOUTUBE_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/youtube.readonly";
const CHANNELS_URL: &str = "https://www.googleapis.com/youtube/v3/channels";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const CALLBACK_LIMIT: usize = 8 * 1024;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(600);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

fn callback_response(accepted: bool) -> String {
    let (status, body) = if accepted {
        (
            "200 OK",
            "已收到授权回调。请返回红果下载，等待频道连接完成；以应用显示的频道信息为准。",
        )
    } else {
        (
            "400 Bad Request",
            "授权回调未通过校验。请返回红果下载查看提示，重新点击授权频道。",
        )
    };
    format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len())
}

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
        .append_pair(
            "scope",
            &format!("{YOUTUBE_UPLOAD_SCOPE} {YOUTUBE_READONLY_SCOPE}"),
        )
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

fn token_rejection(status: reqwest::StatusCode, body: &serde_json::Value) -> AppError {
    // Only classify known error identifiers; never display Google's raw body,
    // description, request URL, or any credential supplied in the exchange.
    match body.get("error").and_then(serde_json::Value::as_str) {
        Some("invalid_client" | "unauthorized_client") => AppError::new(
            "OAUTH_CLIENT_REJECTED",
            "Google 拒绝了客户端凭证，请重新导入对应项目的桌面应用凭证",
        ),
        Some("invalid_grant") => AppError::new(
            "OAUTH_CODE_REJECTED",
            "Google 拒绝了授权码（可能已失效），请重新点击“授权频道”并在新页面授权",
        ),
        _ => AppError::new(
            "OAUTH_TOKEN_EXCHANGE_FAILED",
            format!("Google 令牌接口返回 HTTP {}，请重新授权", status.as_u16()),
        ),
    }
}

fn token_network_error(error: reqwest::Error) -> AppError {
    if error.is_timeout() {
        AppError::new(
            "OAUTH_TOKEN_NETWORK_TIMEOUT",
            "连接 Google 令牌服务超时，请检查系统代理是否可用，然后重新授权",
        )
    } else {
        AppError::new(
            "OAUTH_TOKEN_NETWORK_FAILED",
            "无法连接 Google 令牌服务，请检查网络和系统代理，然后重新授权",
        )
    }
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
    on_callback: Arc<dyn Fn() + Send + Sync>,
}

impl OAuthService {
    pub fn new(config: OAuthClientConfig, vault: Arc<dyn TokenVault>) -> Self {
        Self {
            config,
            client: Client::new(),
            vault,
            opener: system_browser_opener(),
            on_callback: Arc::new(|| {}),
        }
    }

    pub fn with_callback_received(mut self, callback: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.on_callback = callback;
        self
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
        let (mut stream, _) = timeout(CALLBACK_TIMEOUT, listener.accept())
            .await
            .map_err(|_| {
                AppError::new(
                    "OAUTH_CALLBACK_TIMEOUT",
                    "YouTube 授权等待超过 10 分钟，请重新点击“授权频道”，并在新打开的页面完成登录",
                )
            })?
            .map_err(|error| {
                AppError::with_cause("OAUTH_CALLBACK_FAILED", "OAuth 回调失败", error.to_string())
            })?;
        let mut buffer = vec![0u8; CALLBACK_LIMIT];
        let size = timeout(NETWORK_TIMEOUT, stream.read(&mut buffer))
            .await
            .map_err(|_| {
                AppError::new(
                    "OAUTH_CALLBACK_READ_TIMEOUT",
                    "授权回调读取超时，请重新授权",
                )
            })?
            .map_err(|error| {
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
        let response = callback_response(code.is_ok());
        let _ = timeout(NETWORK_TIMEOUT, stream.write_all(response.as_bytes())).await;
        let _ = timeout(NETWORK_TIMEOUT, stream.shutdown()).await;
        drop(stream);
        let code = code?;
        (self.on_callback)();
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
            .timeout(NETWORK_TIMEOUT)
            .form(&fields)
            .send()
            .await
            .map_err(token_network_error)?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .json::<serde_json::Value>()
                .await
                .unwrap_or_default();
            return Err(token_rejection(status, &body));
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
            .timeout(NETWORK_TIMEOUT)
            .query(&[("part", "snippet"), ("mine", "true")])
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| AppError::new("YOUTUBE_CHANNEL_LOOKUP_FAILED", "无法读取 YouTube 频道"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .json::<serde_json::Value>()
                .await
                .unwrap_or_default();
            let reason = body
                .pointer("/error/errors/0/reason")
                .and_then(serde_json::Value::as_str);
            if matches!(reason, Some("accessNotConfigured" | "serviceDisabled")) {
                return Err(AppError::new(
                    "YOUTUBE_API_NOT_ENABLED",
                    "请在凭证所属的 Google Cloud 项目启用 YouTube Data API v3，然后重新授权",
                ));
            }
            if reason == Some("insufficientPermissions") {
                return Err(AppError::new(
                    "YOUTUBE_SCOPE_REQUIRED",
                    "缺少频道读取权限，请重新授权并勾选上传及读取 YouTube 账号权限",
                ));
            }
            return Err(AppError::new(
                "YOUTUBE_CHANNEL_LOOKUP_FAILED",
                format!("无法读取 YouTube 频道（HTTP {}）", status.as_u16()),
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
            .timeout(NETWORK_TIMEOUT)
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
            .timeout(NETWORK_TIMEOUT)
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
    fn token_rejections_classify_known_errors_without_exposing_response_text() {
        for (remote, expected) in [
            ("invalid_client", "OAUTH_CLIENT_REJECTED"),
            ("invalid_grant", "OAUTH_CODE_REJECTED"),
            ("synthetic-secret", "OAUTH_TOKEN_EXCHANGE_FAILED"),
        ] {
            let error = token_rejection(
                reqwest::StatusCode::BAD_REQUEST,
                &serde_json::json!({
                    "error": remote, "error_description": "synthetic-secret"
                }),
            );
            assert_eq!(error.code, expected);
            assert!(!serde_json::to_string(&error)
                .unwrap()
                .contains("synthetic-secret"));
        }
    }

    #[tokio::test]
    async fn valid_callback_notifies_app_before_exchange_and_reports_google_rejection() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let notified = Arc::new(AtomicBool::new(false));
        let token_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let token_port = token_listener.local_addr().unwrap().port();
        let observed = notified.clone();
        let token_server = tokio::spawn(async move {
            let (mut stream, _) = token_listener.accept().await.unwrap();
            assert!(observed.load(Ordering::SeqCst));
            let mut buffer = [0u8; 8192];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = r#"{"error":"invalid_grant","error_description":"synthetic-secret"}"#;
            let reply = format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let mut config = OAuthClientConfig::from_json(r#"{"installed":{"client_id":"synthetic.apps.googleusercontent.com","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://localhost"]}}"#).unwrap();
        config.token_uri = format!("http://127.0.0.1:{token_port}/token");
        let callback_flag = notified.clone();
        let mut service = OAuthService::new(
            config,
            Arc::new(super::super::vault::MemoryTokenVault::default()),
        )
        .with_callback_received(Arc::new(move || {
            callback_flag.store(true, Ordering::SeqCst);
        }))
        .with_opener(Arc::new(|auth_url| {
            let url = Url::parse(auth_url).unwrap();
            let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            let mut redirect = Url::parse(&params["redirect_uri"]).unwrap();
            redirect
                .query_pairs_mut()
                .append_pair("state", &params["state"])
                .append_pair("code", "synthetic-code");
            tokio::spawn(async move {
                let response = Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get(redirect)
                    .send()
                    .await
                    .unwrap();
                assert!(response.text().await.unwrap().contains("等待频道连接完成"));
            });
            Ok(())
        }));
        service.client = Client::builder().no_proxy().build().unwrap();
        let error = timeout(Duration::from_secs(5), service.authorize())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, "OAUTH_CODE_REJECTED");
        assert!(notified.load(Ordering::SeqCst));
        assert!(!error.message.contains("synthetic-secret"));
        token_server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "manual network probe; requires Google connectivity through current system settings"]
    async fn google_token_endpoint_is_reachable_with_system_network_settings() {
        // Deliberately send no credential, code, or token. HTTP 400 proves that
        // the same default HTTP client used by OAuth can reach Google.
        let response = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap()
            .post("https://oauth2.googleapis.com/token")
            .form(&[("grant_type", "authorization_code")])
            .send()
            .await;
        assert!(response.is_ok(), "Google token endpoint was unreachable");
        assert_eq!(response.unwrap().status(), reqwest::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn callback_receipt_does_not_claim_connection_success_and_has_exact_byte_length() {
        for accepted in [true, false] {
            let response = callback_response(accepted);
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            assert!(headers.contains(&format!("Content-Length: {}", body.len())));
            assert!(headers.contains("Connection: close"));
            assert!(headers.contains("Cache-Control: no-store"));
            assert!(!body.contains("Authorization complete"));
            if accepted {
                assert!(body.contains("等待频道连接完成"));
            } else {
                assert!(headers.starts_with("HTTP/1.1 400"));
            }
        }
    }

    #[test]
    fn authorization_url_uses_pkce_state_loopback_and_upload_and_readonly_scopes() {
        let config = OAuthClientConfig::from_json(r#"{"installed":{"client_id":"synthetic.apps.googleusercontent.com","client_secret":"secret","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://127.0.0.1"]}}"#).unwrap();
        let request =
            build_authorization_request(&config, 49152, "state-value", "verifier-value").unwrap();
        let query = request
            .url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("scope").map(|value| value.as_ref()),
            Some(format!("{YOUTUBE_UPLOAD_SCOPE} {YOUTUBE_READONLY_SCOPE}").as_str())
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
