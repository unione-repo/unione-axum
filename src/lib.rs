//! UniOne Web API for [axum](https://docs.rs/axum): the client of the `unione-rust-sdk` crate configured
//! from the environment and the webhook endpoint.
//!
//! ```no_run
//! use axum::{routing::get, Extension, Router};
//! use unione_wrapper::Config;
//!
//! # async fn run() {
//! let config = Config::from_env();
//! let app: Router = Router::new()
//!     .route("/", get(|| async { "ok" }))
//!     .layer(Extension(config.client()));
//! # }
//! ```

use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use md5::{Digest, Md5};

use unione::api::webhook::model::{
    WebhookCallback, WebhookCallbackEventsByUserItem, WebhookCallbackEventsByUserItemEventsItem,
};
use unione::api::webhook::WebhookApi;
use unione::client::base_url_for;
use unione::{Configuration, UniOneClient};

/// The crate and its version in the "platform" query parameter of the requests, before the SDK one.
pub const PLATFORM: &str = "axum.1.2.3";

/// Environment variable of the API key.
pub const ENV_API_KEY: &str = "UNIONE_API_KEY";
/// Environment variable of the API host (datacenter) or full base URL.
pub const ENV_HOST: &str = "UNIONE_HOST";
/// Environment variable of the request timeout, seconds.
pub const ENV_TIMEOUT: &str = "UNIONE_TIMEOUT";
/// Environment variable of the API key the webhooks are signed with.
pub const ENV_WEBHOOK_KEY: &str = "UNIONE_WEBHOOK_KEY";
/// Environment variable of the path of the webhook endpoint.
pub const ENV_WEBHOOK_PATH: &str = "UNIONE_WEBHOOK_PATH";
/// Path of the webhook endpoint when none is configured.
pub const DEFAULT_WEBHOOK_PATH: &str = "/unione/webhook";
/// Largest webhook request body accepted, bytes.
pub const MAX_WEBHOOK_BODY: usize = 16 << 20;

/// Error of a webhook handler: the endpoint answers 500, so the API repeats the notification later.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Error of an axum handler that calls the API: `?` turns any error (of the client, of I/O...) into it, and
/// it answers 502 Bad Gateway with the message of the error.
///
/// ```no_run
/// use axum::Extension;
/// use unione::UniOneClient;
/// use unione_wrapper::HandlerError;
///
/// async fn ping(Extension(client): Extension<UniOneClient>) -> Result<(), HandlerError> {
///     client.system().ping().await?;
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct HandlerError(pub BoxError);

impl<E> From<E> for HandlerError
where
    E: Error + Send + Sync + 'static,
{
    fn from(error: E) -> Self {
        HandlerError(Box::new(error))
    }
}

impl IntoResponse for HandlerError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_GATEWAY, self.0.to_string()).into_response()
    }
}

/// Settings of the client and of the webhook endpoint.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// User or project API key.
    pub api_key: String,
    /// API host (datacenter) or a full base URL; `None`: the default host.
    pub host: Option<String>,
    /// Request timeout; `None`: 30 seconds.
    pub timeout: Option<Duration>,
    /// API key the webhooks are signed with; `None`: `api_key`.
    pub webhook_key: Option<String>,
    /// Path of the webhook endpoint; `None`: [`DEFAULT_WEBHOOK_PATH`].
    pub webhook_path: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Config {
    /// Reads the `UNIONE_*` environment variables (`UNIONE_TIMEOUT` in seconds).
    pub fn from_env() -> Self {
        Config {
            api_key: env(ENV_API_KEY).unwrap_or_default(),
            host: env(ENV_HOST),
            timeout: env(ENV_TIMEOUT)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|s| *s > 0.0)
                .map(Duration::from_secs_f64),
            webhook_key: env(ENV_WEBHOOK_KEY),
            webhook_path: env(ENV_WEBHOOK_PATH),
        }
    }

    /// API client of the configuration.
    pub fn client(&self) -> UniOneClient {
        let mut config = Configuration::new(self.api_key.clone());
        config.platform = Some(PLATFORM.to_string());
        if let Some(host) = &self.host {
            config.base_url = base_url_for(host);
        }
        if let Some(timeout) = self.timeout {
            config.timeout = timeout;
        }
        UniOneClient::with_config(config)
    }

    /// Key the webhooks are checked with: `webhook_key` or `api_key`.
    pub fn signing_key(&self) -> &str {
        self.webhook_key.as_deref().unwrap_or(&self.api_key)
    }

    /// Path of the webhook endpoint.
    pub fn webhook_path(&self) -> &str {
        self.webhook_path.as_deref().unwrap_or(DEFAULT_WEBHOOK_PATH)
    }
}

/// Receives the webhook notifications with a valid signature. Both methods have empty defaults.
/// They must be fast: the API waits for 200 OK for 3 seconds.
pub trait WebhookHandler: Send + Sync + 'static {
    /// The whole notification.
    fn on_notification(
        &self,
        notification: &WebhookCallback,
    ) -> impl Future<Output = Result<(), BoxError>> + Send {
        let _ = notification;
        async { Ok(()) }
    }

    /// One event of the notification (email status, spam block...) with its user/project.
    fn on_event(
        &self,
        event: &WebhookCallbackEventsByUserItemEventsItem,
        user: &WebhookCallbackEventsByUserItem,
    ) -> impl Future<Output = Result<(), BoxError>> + Send {
        let _ = (event, user);
        async { Ok(()) }
    }
}

struct WebhookState<H> {
    key: String,
    handler: H,
}

/// Router with the webhook endpoint at `config.webhook_path()`: GET/HEAD answer 200 (the URL check of
/// webhook/set), POST checks the signature (401 when wrong), parses the notification and calls the handler.
/// Merge it into the application: `app.merge(webhook_router(&config, handler))`.
pub fn webhook_router<S, H>(config: &Config, handler: H) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    H: WebhookHandler,
{
    let state = Arc::new(WebhookState {
        key: config.signing_key().to_string(),
        handler,
    });
    Router::new()
        .route(
            config.webhook_path(),
            get(|| async { "OK" }).post(receive::<H>),
        )
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY))
        .with_state(state)
}

async fn receive<H: WebhookHandler>(
    State(state): State<Arc<WebhookState<H>>>,
    body: Bytes,
) -> Response {
    let Ok(text) = std::str::from_utf8(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid body").into_response();
    };
    if !verify_signature(text, &state.key) {
        return (StatusCode::UNAUTHORIZED, "invalid signature").into_response();
    }
    let Ok(notification) = WebhookApi::parse_callback(text) else {
        return (StatusCode::BAD_REQUEST, "invalid body").into_response();
    };
    match dispatch(&state.handler, &notification).await {
        Ok(()) => "OK".into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "webhook handler failed").into_response(),
    }
}

async fn dispatch<H: WebhookHandler>(
    handler: &H,
    notification: &WebhookCallback,
) -> Result<(), BoxError> {
    handler.on_notification(notification).await?;
    for user in notification.events_by_user.iter().flatten() {
        for event in user.events.iter().flatten() {
            handler.on_event(event, user).await?;
        }
    }
    Ok(())
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn auth_value(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let auth = value.get("auth")?.as_str()?;
    (auth.len() == 32 && auth.bytes().all(|b| b.is_ascii_hexdigit())).then(|| auth.to_string())
}

/// Checks the `auth` field of a webhook notification: the MD5 of the request body in which the `auth` value
/// is replaced by the API key of the user/project of the webhook.
pub fn verify_signature(body: &str, api_key: &str) -> bool {
    let Some(auth) = auth_value(body) else {
        return false;
    };
    if api_key.is_empty() {
        return false;
    }
    let expected = md5_hex(body.replace(&auth, api_key).as_bytes());
    let given = auth.to_ascii_lowercase();
    // constant time comparison
    expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// The `auth` value [`sign`] replaces.
pub const SIGNATURE_PLACEHOLDER: &str = "00000000000000000000000000000000";

/// Signs a body whose `auth` is [`SIGNATURE_PLACEHOLDER`], e.g. for tests.
pub fn sign(body: &str, api_key: &str) -> String {
    let auth = md5_hex(body.replace(SIGNATURE_PLACEHOLDER, api_key).as_bytes());
    body.replace(SIGNATURE_PLACEHOLDER, &auth)
}
