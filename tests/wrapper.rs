//! Tests of the crate: the client against a mock of the API on 127.0.0.1, the webhook endpoint
//! through the router. No network, no real API.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use tower::ServiceExt;

use unione::api::webhook::model::{
    WebhookCallback, WebhookCallbackEventsByUserItem, WebhookCallbackEventsByUserItemEventsItem,
};
use unione::client;
use unione_wrapper::{
    sign, verify_signature, webhook_router, BoxError, Config, HandlerError, WebhookHandler,
    DEFAULT_WEBHOOK_PATH, ENV_API_KEY, ENV_HOST, ENV_TIMEOUT, ENV_WEBHOOK_PATH, PLATFORM,
};

const KEY: &str = "test-key";
const NO_API: &str = "http://127.0.0.1:9";
const PONG: &str = r#"{"status":"success","user_id":1}"#;
const API_ERROR: &str = r#"{"status":"error","message":"Unknown recipient","code":204}"#;
/// A webhook notification with two events; `sign` replaces its auth placeholder.
const SAMPLE: &str = r#"{
    "auth": "00000000000000000000000000000000",
    "events_by_user": [
        {
            "user_id": 1,
            "project_id": "6432890213745872",
            "events": [
                {
                    "event_name": "transactional_email_status",
                    "event_data": {
                        "job_id": "1a3Q2V-0000OZ-S0",
                        "email": "user@example.com",
                        "status": "delivered",
                        "event_time": "2015-11-30 15:09:42"
                    }
                },
                {
                    "event_name": "transactional_spam_block",
                    "event_data": {
                        "block_time": "2015-11-30 15:09:42",
                        "block_type": "one_smtp",
                        "domain": "example.com",
                        "SMTP_blocks_count": 1,
                        "domain_status": "blocked"
                    }
                }
            ]
        }
    ]
}"#;
const FAILED: &str = "email sending failed";

/// A request the mock received.
#[derive(Clone)]
struct Received {
    line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        let (_, value) = self.headers.iter().find(|(key, _)| key == name)?;
        Some(value.as_str())
    }
}

/// Mock of the API on 127.0.0.1: answers each request with the status and the body after the delay.
struct Mock {
    url: String,
    log: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start(status: u16, body: &str) -> Mock {
        Mock::with_delay(status, body, Duration::ZERO)
    }

    fn with_delay(status: u16, body: &str, delay: Duration) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}{}", client::BASE_PATH);
        let log = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&log);
        let answer = response(status, body);
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                if let Some(request) = read_request(&mut stream) {
                    received.lock().unwrap().push(request);
                }
                thread::sleep(delay);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        Mock { url, log }
    }

    fn received(&self) -> Vec<Received> {
        self.log.lock().unwrap().clone()
    }
}

/// HTTP/1.1 response of the mock: JSON or an HTML page (of a proxy).
fn response(status: u16, body: &str) -> String {
    let kind = if body.starts_with('{') {
        "application/json"
    } else {
        "text/html"
    };
    let length = body.len();
    let mut head = format!("HTTP/1.1 {status} Mock\r\nContent-Type: {kind}\r\n");
    head += &format!("Content-Length: {length}\r\nConnection: close\r\n\r\n");
    head + body
}

/// Reads a request: the head, then the body of its Content-Length.
fn read_request(stream: &mut TcpStream) -> Option<Received> {
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout).ok()?;
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_string();
    let mut headers = Vec::new();
    for header in lines {
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            headers.push((name, value.trim().to_string()));
        }
    }
    let mut received = Received {
        line,
        headers,
        body: String::new(),
    };
    let length = received
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    received.body = String::from_utf8_lossy(&data[head_end..]).into_owned();
    Some(received)
}

/// Settings with the client of the mock (the webhook tests call no API).
fn config(url: &str) -> Config {
    Config {
        api_key: KEY.to_string(),
        host: Some(url.to_string()),
        ..Config::default()
    }
}

/// Status and body of the response of a handler error.
async fn answer(error: HandlerError) -> (StatusCode, String) {
    let response = error.into_response();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[derive(Clone, Default)]
struct Counter {
    notifications: Arc<AtomicUsize>,
    events: Arc<AtomicUsize>,
    email: Arc<Mutex<Option<String>>>,
}

impl WebhookHandler for Counter {
    async fn on_notification(&self, _notification: &WebhookCallback) -> Result<(), BoxError> {
        self.notifications.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_event(
        &self,
        event: &WebhookCallbackEventsByUserItemEventsItem,
        _user: &WebhookCallbackEventsByUserItem,
    ) -> Result<(), BoxError> {
        if self.events.fetch_add(1, Ordering::SeqCst) == 0 {
            *self.email.lock().unwrap() = event.event_data.as_ref().and_then(|d| d.email.clone());
        }
        Ok(())
    }
}

async fn call(app: &Router, method: &str, path: &str, body: String) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[test]
fn config_reads_the_environment() {
    std::env::set_var(ENV_API_KEY, KEY);
    std::env::set_var(ENV_HOST, "http://127.0.0.1:9/api");
    std::env::set_var(ENV_TIMEOUT, "2.5");
    std::env::set_var(ENV_WEBHOOK_PATH, "/hooks/mail");
    let settings = Config::from_env();
    assert_eq!(settings.api_key, KEY);
    assert_eq!(settings.host.as_deref(), Some("http://127.0.0.1:9/api"));
    assert_eq!(settings.timeout, Some(Duration::from_millis(2500)));
    assert_eq!(settings.webhook_path(), "/hooks/mail");
}

#[test]
fn signing_key_defaults_to_the_api_key() {
    let mut settings = config(NO_API);
    assert_eq!(settings.signing_key(), KEY);
    assert_eq!(settings.webhook_path(), DEFAULT_WEBHOOK_PATH);
    settings.webhook_key = Some("hook-key".to_string());
    assert_eq!(settings.signing_key(), "hook-key");
}

#[tokio::test]
async fn client_sends_the_key_and_the_platform() {
    let mock = Mock::start(200, PONG);
    let response = config(&mock.url).client().system().ping().await;
    assert_eq!(response.unwrap().user_id, Some(1));
    let received = mock.received();
    let platform = format!("{PLATFORM},{}", client::PLATFORM);
    let path = format!("{}/system/ping.json", client::BASE_PATH);
    let line = format!("POST {path}?platform={platform} HTTP/1.1");
    assert_eq!(received[0].line, line);
    assert_eq!(received[0].header("x-api-key"), Some(KEY));
    assert_eq!(received[0].body, "{}");
}

#[tokio::test]
async fn api_error_answers_bad_gateway() {
    let mock = Mock::start(400, API_ERROR);
    let api = config(&mock.url).client();
    let error = api.system().ping().await.unwrap_err();
    let (status, body) = answer(error.into()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body, FAILED);
}

#[tokio::test]
async fn timeout_answers_gateway_timeout() {
    let mock = Mock::with_delay(200, PONG, Duration::from_secs(5));
    let mut settings = config(&mock.url);
    settings.timeout = Some(Duration::from_millis(200));
    let error = settings.client().system().ping().await.unwrap_err();
    let (status, body) = answer(error.into()).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(body, FAILED);
}

#[tokio::test]
async fn invalid_request_answers_internal_server_error() {
    let mut settings = config(NO_API);
    settings.api_key = String::new();
    let error = settings.client().system().ping().await.unwrap_err();
    let (status, body) = answer(error.into()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, FAILED);
}

#[tokio::test]
async fn other_errors_answer_bad_gateway() {
    let error = std::io::Error::other("connection reset");
    let (status, body) = answer(error.into()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body, FAILED);
}

#[tokio::test]
async fn signed_notification_reaches_the_handler() {
    let counter = Counter::default();
    let app: Router = webhook_router(&config(NO_API), counter.clone());
    let path = DEFAULT_WEBHOOK_PATH;
    let get = call(&app, "GET", path, String::new()).await;
    assert_eq!(get, StatusCode::OK);
    let post = call(&app, "POST", path, sign(SAMPLE, KEY)).await;
    assert_eq!(post, StatusCode::OK);
    assert_eq!(counter.notifications.load(Ordering::SeqCst), 1);
    assert_eq!(counter.events.load(Ordering::SeqCst), 2);
    let email = counter.email.lock().unwrap().clone();
    assert_eq!(email.as_deref(), Some("user@example.com"));
}

#[tokio::test]
async fn wrong_signature_is_unauthorized() {
    let counter = Counter::default();
    let app: Router = webhook_router(&config(NO_API), counter.clone());
    let path = DEFAULT_WEBHOOK_PATH;
    let forged = sign(SAMPLE, KEY).replacen("user@", "evil@", 1);
    let status = call(&app, "POST", path, forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let unsigned = call(&app, "POST", path, SAMPLE.to_string()).await;
    assert_eq!(unsigned, StatusCode::UNAUTHORIZED);
    let other_key = call(&app, "POST", path, sign(SAMPLE, "other-key")).await;
    assert_eq!(other_key, StatusCode::UNAUTHORIZED);
    assert_eq!(counter.notifications.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn webhook_key_and_path_are_configurable() {
    let mut settings = config(NO_API);
    settings.webhook_key = Some("hook-key".to_string());
    settings.webhook_path = Some("/hooks/mail".to_string());
    let app: Router = webhook_router(&settings, Counter::default());
    let path = "/hooks/mail";
    let signed = call(&app, "POST", path, sign(SAMPLE, "hook-key")).await;
    assert_eq!(signed, StatusCode::OK);
    let api_key = call(&app, "POST", path, sign(SAMPLE, KEY)).await;
    assert_eq!(api_key, StatusCode::UNAUTHORIZED);
    let default_path = DEFAULT_WEBHOOK_PATH;
    let moved = call(&app, "GET", default_path, String::new()).await;
    assert_eq!(moved, StatusCode::NOT_FOUND);
}

#[test]
fn sign_and_verify_signature() {
    let body = sign(SAMPLE, KEY);
    assert!(verify_signature(&body, KEY));
    assert!(!verify_signature(&body, "other-key"));
    assert!(!verify_signature(&body, ""));
    assert!(!verify_signature(SAMPLE, KEY));
}
