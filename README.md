# UniOne axum 1.1.2

Web API 1.90

Quickstart guide: [Send emails with Rust](https://docs.unione.io/en/send-with-rust) · Docs for LLMs: [llms.txt](https://docs.unione.io/llms.txt) · [API reference](https://docs.unione.io/en/web-api-ref)

axum integration of the [UniOne Rust SDK](https://github.com/unione-repo/unione-rust-sdk) (`unione-rust-sdk`): the API client in the axum state / `Extension`, a webhook endpoint with signature check.

## Install

```bash
cargo add unione-axum unione-rust-sdk
```

Requirements: Rust 1.75+, axum 0.7, tokio.

## Get an API key

1. Sign up at [cp.unione.io](https://cp.unione.io/en/user/registration/).
2. Copy your API key from [Settings → Security → API](https://cp.unione.io/en/settings/security/api).
3. Put it in an environment variable:

```bash
export UNIONE_API_KEY="your-api-key"
```

The package reads it from `UNIONE_API_KEY` (see the configuration below).

> **Server-side only.** Use this package in backend code: API routes, server functions, workers.
> Never ship the API key to a browser or a mobile app, and never commit it.

## Configuration

Settings are read from the environment by default:

```bash
UNIONE_API_KEY=YOUR_API_KEY
UNIONE_HOST=us1.unione.io
```

| Option | Environment variable | Default | Description |
|---|---|---|---|
| `api_key` | `UNIONE_API_KEY` |  | User or project API key (required). |
| `host` | `UNIONE_HOST` | `api.unione.io` | API host (datacenter: `api.unione.io`, `eu1.unione.io`, `us1.unione.io`) or a full base URL. |
| `timeout` | `UNIONE_TIMEOUT` | `30` | Request timeout, seconds. |
| `webhook_key` | `UNIONE_WEBHOOK_KEY` |  | API key the webhooks are signed with (the key of the user/project of the webhook), if it differs from the API key. |
| `webhook_path` | `UNIONE_WEBHOOK_PATH` | `/unione/webhook` | Path of the webhook endpoint. |

`Config::from_env()` reads the variables; the fields of `Config` can also be set in code.

## Send your first email — no DNS setup

Every account can send test email right away from a **sandbox domain**: pre-verified, DKIM and SPF already set.
It delivers only to addresses you confirm.

The examples use the API client of the package (see the configuration above).

**1. Get your sandbox domain** (`domain/get-sandbox-domain`). The call creates the domain on first use and returns the same one afterwards.

```rust
use axum::Extension;
use unione::UniOneClient;

async fn send(Extension(client): Extension<UniOneClient>) -> Result<String, String> {
    let response = client.domain().get_sandbox_domain().await.map_err(|e| e.to_string())?;
    Ok(format!("{:?}", response.domain))
}
```

**2. Confirm your inbox** (`confirmed-email/create`). UniOne sends a confirmation email; click the link in it within 48 hours.

```rust
use axum::Extension;
use unione::UniOneClient;
use unione::api::confirmed_email::model::ConfirmedEmailRequest;

async fn send(Extension(client): Extension<UniOneClient>) -> Result<String, String> {
    let response = client.confirmed_email().create(&ConfirmedEmailRequest {
        email: "you@example.com".to_string(),
    }).await.map_err(|e| e.to_string())?;
    Ok(format!("{:?}", response.status))
}
```

Check the status with `confirmed-email/list` — your address should show `"status": "confirmed"`.

**3. Send the email** (`email/send`) from any address on the sandbox domain to the confirmed address.

```rust
use axum::Extension;
use unione::UniOneClient;
use unione::api::email::model::EmailSendRequest;
use unione::api::email::model::MessageObject;
use unione::api::email::model::MessageObjectRecipientsItem;
use unione::common::model::BodyObject;

async fn send(Extension(client): Extension<UniOneClient>) -> Result<String, String> {
    let response = client.email().send(&EmailSendRequest {
        message: MessageObject {
            recipients: vec![
                MessageObjectRecipientsItem {
                    email: "you@example.com".to_string(),
                    ..Default::default()
                },
            ],
            body: BodyObject {
                html: Some("<p>It works!</p>".to_string()),
                ..Default::default()
            },
            subject: "Hello from UniOne".to_string(),
            from_email: Some("noreply@<sandbox-domain>".to_string()),
            ..Default::default()
        },
    }).await.map_err(|e| e.to_string())?;
    Ok(format!("{:?}", response.job_id))
}
```

### What you get back

```json
{
  "status": "success",
  "job_id": "1ZymBc-00041N-9X",
  "emails": [
    "you@example.com"
  ]
}
```

On error the API returns `status: "error"`, a `message` and a numeric `code`:

```json
{
  "status": "error",
  "code": 102,
  "message": "Error ID:83274F98-C7D5-11F1-B221-6214FF8707B1. Can not decode key wrongkey"
}
```

### Same flow with curl

```bash
BASE=https://api.unione.io/en/transactional/api/v1

curl -s -X POST $BASE/domain/get-sandbox-domain.json \
  -H "Content-Type: application/json" -H "X-API-KEY: $UNIONE_API_KEY" -d '{}'

curl -s -X POST $BASE/confirmed-email/create.json \
  -H "Content-Type: application/json" -H "X-API-KEY: $UNIONE_API_KEY" \
  -d '{"email": "you@example.com"}'

curl -s -X POST $BASE/email/send.json \
  -H "Content-Type: application/json" -H "X-API-KEY: $UNIONE_API_KEY" \
  -d '{
        "message": {
          "recipients": [
            {
              "email": "you@example.com"
            }
          ],
          "from_email": "noreply@<sandbox-domain>",
          "subject": "Hello from UniOne",
          "body": {
            "html": "<p>It works!</p>"
          }
        }
      }'
```

## Webhooks

The endpoint answers `GET` with 200 OK (the URL check of [webhook/set](https://docs.unione.io/en/web-api-ref#webhook-set)) and accepts the `POST` notifications: it checks the `auth` field (MD5 of the request body in which the `auth` value is replaced by the API key), answers 401 to a wrong signature, and passes the notification and each of its events to the application.

```rust
use unione::api::webhook::model::{
    WebhookCallbackEventsByUserItem, WebhookCallbackEventsByUserItemEventsItem,
};
use unione_wrapper::{webhook_router, BoxError, Config, WebhookHandler};

struct Events;

impl WebhookHandler for Events {
    async fn on_event(
        &self,
        event: &WebhookCallbackEventsByUserItemEventsItem,
        _user: &WebhookCallbackEventsByUserItem,
    ) -> Result<(), BoxError> {
        if let Some(data) = &event.event_data {
            println!("UniOne event {:?}: {:?} {:?}", event.event_name, data.email, data.status);
        }
        Ok(())
    }
}

let config = Config::from_env();
let app = app.merge(webhook_router(&config, Events));
```

Register the URL of the endpoint with `webhook/set`, e.g. `https://example.com/unione/webhook`.

## API client

Every section of the API is available on the client, see the [SDK docs](https://github.com/unione-repo/unione-rust-sdk).

```rust
use axum::{routing::get, Extension, Router};
use unione::UniOneClient;
use unione_wrapper::Config;

async fn usage(Extension(client): Extension<UniOneClient>) -> String {
    match client.system().info().await {
        Ok(info) => format!("{:?}", info.email),
        Err(e) => e.to_string(),
    }
}

let config = Config::from_env();
let app = Router::new()
    .route("/usage", get(usage))
    .layer(Extension(config.client()));
```

## Sending through the API

The client calls [email/send](https://docs.unione.io/en/web-api-ref#email-send) with all of its options: templates, substitutions, metadata, tracking. Fields of the request and other examples: [Rust SDK docs](https://github.com/unione-repo/unione-rust-sdk/blob/HEAD/docs/Email.md#send).

```rust
use axum::Extension;
use unione::UniOneClient;
use std::collections::HashMap;
use unione::api::email::model::EmailSendRequest;
use unione::api::email::model::MessageObject;
use unione::api::email::model::MessageObjectRecipientsItem;
use unione::common::model::BodyObject;

async fn send(Extension(client): Extension<UniOneClient>) -> Result<String, String> {
    let response = client.email().send(&EmailSendRequest {
        message: MessageObject {
            recipients: vec![
                MessageObjectRecipientsItem {
                    email: "user@example.com".to_string(),
                    substitutions: Some(HashMap::from([
                        ("CustomerId".to_string(), "12452".to_string()),
                        ("to_name".to_string(), "John Smith".to_string()),
                    ])),
                    ..Default::default()
                },
            ],
            body: BodyObject {
                html: Some("<b>Hello, {{to_name}}</b>".to_string()),
                plaintext: Some("Hello, {{to_name}}".to_string()),
                ..Default::default()
            },
            subject: "UniOne test email ".to_string(),
            from_email: Some("email@example.com".to_string()),
            from_name: Some("John Smith".to_string()),
            ..Default::default()
        },
    }).await.map_err(|e| e.to_string())?;
    Ok(format!("{:?}", response.job_id))
}
```

## Sandbox limits

- Sends only to confirmed addresses: up to 10 per account.
- Up to 100 emails a day.
- The sandbox domain expires after 31 days; call `domain/get-sandbox-domain` again to get a new one.
- Works only with the account API key, not a project key.
- Not listed in `domain/list`; the other `domain/*` methods do not apply to it.

The sandbox is for development and testing. Do not use a sandbox domain as the sender in production.

## Before production: verify your domain

1. `domain/get-dns-records` — returns the values for your DNS records.
2. Add these records at your DNS provider (the domain page of the control panel shows them too):

   | Type | Host | Value |
   |---|---|---|
   | TXT | `@` | the `verification-record` value (`unione-validate-hash=…`) |
   | TXT | `<selector>._domainkey` (the selector is on the domain page of the control panel, e.g. `us`) | `v=DKIM1; k=rsa; p=<dkim>` |
   | TXT | `@` | `v=spf1 include:spf.unione.io ~all` |
   | CNAME | `_dmarc` | the `dmarc` value |

3. `domain/validate-verification-record`, then `domain/validate-dkim`.
4. `domain/list` — wait until the verification record is confirmed and DKIM is active.
5. Read `from_email` from your config, so switching from the sandbox needs no code change.

## Sending reliably

- **Retries:** set `message.idempotence_key` on sends that may be retried, so a repeated request does not send twice.
- **Limits:** up to 500 recipients and 10 MB per request; each attachment up to 7 MB.
- **Body:** always include `body.html` or `body.plaintext`.
- **Delivery status:** register a webhook (`webhook/set`) with a fast, idempotent HTTPS handler instead of polling.
- **Defaults:** keep the default unsubscribe footer and tracking; changing them requires UniOne support.
- **Errors:** retry `429` and `5xx` with backoff; do not retry other errors unchanged.

## Links

- [Send emails with Rust](https://docs.unione.io/en/send-with-rust) and other [integration guides](https://docs.unione.io/en/integrations/)
- [Web API reference](https://docs.unione.io/en/web-api-ref)
- [llms.txt](https://docs.unione.io/llms.txt) for AI coding agents
- [UniOne MCP server and agent setup](https://unione.io/en/agents)
- [Support](https://cp.unione.io/en/support)

## For package contributors

See [AGENTS.md](AGENTS.md): how this package is generated from the OpenAPI spec, built, tested and released.
