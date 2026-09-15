use crate::{app_error::AppError, AppResult};
use reqwest::blocking::Client;
use serde_json::Value;
use std::{
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

const API_CONTRACT: &str = "hongguo-desktop-v2";
// One-file Python bundles may take longer on a cold disk or during antivirus scans.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct ApiReadiness {
    ready: OnceLock<()>,
    last_check: Mutex<Option<(Instant, AppResult<()>)>>,
}

impl ApiReadiness {
    fn ensure(&self, client: &Client, base: &str, timeout: Duration) -> AppResult<()> {
        if self.ready.get().is_some() {
            return Ok(());
        }
        let requested_at = Instant::now();
        let mut last = self.last_check.lock().unwrap();
        if self.ready.get().is_some() {
            return Ok(());
        }
        // Concurrent callers share the check they waited for, including its error.
        // A later retry must perform a new check instead of caching failure forever.
        if let Some((finished_at, result)) = &*last {
            if *finished_at >= requested_at {
                return result.clone();
            }
        }
        let result = wait_for_health(client, base, timeout);
        if result.is_ok() {
            let _ = self.ready.set(());
        }
        *last = Some((Instant::now(), result.clone()));
        result
    }
}

pub(crate) fn ensure_api_ready(client: &Client, base: &str, ready: &ApiReadiness) -> AppResult<()> {
    ready.ensure(client, base, STARTUP_TIMEOUT)
}

fn health_matches_contract(value: &Value) -> bool {
    value.get("status").and_then(Value::as_str) == Some("ok")
        && value.get("api_contract").and_then(Value::as_str) == Some(API_CONTRACT)
}

fn wait_for_health(client: &Client, base: &str, timeout: Duration) -> AppResult<()> {
    let start = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(AppError::new(
                "API_STARTUP_TIMEOUT",
                "本地 API 暂未就绪，请稍后点击重试；服务状态会自动重新检查",
            ));
        }
        if let Ok(response) = client
            .get(format!("{base}/health"))
            .timeout(remaining.min(Duration::from_secs(2)))
            .send()
        {
            if response.status().is_success() {
                if let Ok(value) = response.json::<Value>() {
                    if health_matches_contract(&value) {
                        return Ok(());
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100).min(timeout.saturating_sub(start.elapsed())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{mpsc, Arc, Barrier},
    };

    #[test]
    fn health_requires_the_desktop_contract() {
        assert!(health_matches_contract(&serde_json::json!({
            "status": "ok", "api_contract": "hongguo-desktop-v2"
        })));
        assert!(!health_matches_contract(
            &serde_json::json!({"status": "ok"})
        ));
    }

    #[test]
    fn timeout_bounds_a_server_that_accepts_without_responding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (_connection, _) = listener.accept().unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(2));
        });
        let client = Client::builder().no_proxy().build().unwrap();
        let start = Instant::now();
        let result = wait_for_health(&client, &base, Duration::from_millis(80));
        let elapsed = start.elapsed();
        release.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(result.unwrap_err().code, "API_STARTUP_TIMEOUT");
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn retry_recovers_after_startup_timeout_without_restarting_the_app() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let client = Client::builder().no_proxy().build().unwrap();
        let ready = ApiReadiness::default();
        assert!(ready.ensure(&client, &base, Duration::ZERO).is_err());
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut connection)
                .read_line(&mut request)
                .unwrap();
            assert!(request.starts_with("GET /health "));
            let body = r#"{"status":"ok","api_contract":"hongguo-desktop-v2"}"#;
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        ready
            .ensure(&client, &base, Duration::from_secs(2))
            .unwrap();
        server.join().unwrap();
        // Readiness remains shared after success, without needing another server.
        ready.ensure(&client, &base, Duration::ZERO).unwrap();
    }

    #[test]
    fn simultaneous_pages_share_one_successful_startup_check() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let ready = Arc::new(ApiReadiness::default());
        let barrier = Arc::new(Barrier::new(6));
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut connection)
                .read_line(&mut request)
                .unwrap();
            assert!(request.starts_with("GET /health "));
            thread::sleep(Duration::from_millis(100));
            let body = r#"{"status":"ok","api_contract":"hongguo-desktop-v2"}"#;
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let callers: Vec<_> = (0..6)
            .map(|_| {
                let (ready, barrier, base) = (ready.clone(), barrier.clone(), base.clone());
                thread::spawn(move || {
                    let client = Client::builder().no_proxy().build().unwrap();
                    barrier.wait();
                    ready
                        .ensure(&client, &base, Duration::from_secs(2))
                        .unwrap();
                })
            })
            .collect();
        for caller in callers {
            caller.join().unwrap();
        }
        server.join().unwrap();
    }
}
