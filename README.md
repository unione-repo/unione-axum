# UniOne axum

[![axum](https://img.shields.io/crates/v/unione-axum)](https://crates.io/crates/unione-axum) [![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

axum integration of the [UniOne Rust SDK](https://github.com/unione-repo/unione-rust-sdk) (`unione-rust-sdk`): the API client in the axum state / `Extension`, a webhook endpoint with signature check.

[API reference](https://docs.unione.io/en/web-api-ref) · Docs for LLMs: [llms.txt](https://docs.unione.io/llms.txt)

## Install

```bash
cargo add unione-axum unione-rust-sdk
```

Requirements: Rust 1.75+, axum 0.7, tokio.

## Configuration

Get an API key in the [control panel](https://cp.unione.io/en/settings/security/api) and keep it in the environment, never in code:

```bash
export UNIONE_API_KEY="your-api-key"
```

Settings are read from the environment by default:

```bash
UNIONE_API_KEY=YOUR_API_KEY
UNIONE_HOST=us1.unione.io
```

`us1.unione.io` in the example is for an account on another datacenter; without the host setting the package uses `api.unione.io`.

| Option | Environment variable | Default | Description |
|---|---|---|---|
| `api_key` | `UNIONE_API_KEY` |  | User or project API key (required). |
| `host` | `UNIONE_HOST` | `api.unione.io` | API host (datacenter: `api.unione.io`, `eu1.unione.io`, `us1.unione.io`) or a full base URL. |
| `timeout` | `UNIONE_TIMEOUT` | `30` | Request timeout, seconds. |
| `webhook_key` | `UNIONE_WEBHOOK_KEY` |  | API key the webhooks are signed with (the key of the user/project of the webhook), if it differs from the API key. |
| `webhook_path` | `UNIONE_WEBHOOK_PATH` | `/unione/webhook` | Path of the webhook endpoint. |

`Config::from_env()` reads the variables; the fields of `Config` can also be set in code.

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

> **Answer 200 within 3 seconds** and process the events in the background (a queue): otherwise UniOne repeats the request every 10 minutes for 24 hours (`retry_count` grows), and a handler that failed every request for at least 10 events of a day is turned inactive. The handler must take at least **5 parallel requests of up to 100 events each**; deduplicate, a repeat delivers the same events again.

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

### Sending through the API

The client calls [email/send](https://docs.unione.io/en/web-api-ref#email-send) with all of its options: templates, substitutions, metadata, tracking. Fields of the request and other examples: [Rust SDK docs](https://github.com/unione-repo/unione-rust-sdk/blob/HEAD/docs/Email.md#send).

```rust
use axum::Extension;
use unione::api::email::model::EmailSendRequest;
use unione::api::email::model::MessageObject;
use unione::api::email::model::MessageObjectRecipientsItem as Recipient;
use unione::common::model::BodyObject;
use unione::UniOneClient;
use unione_wrapper::HandlerError;

async fn send(Extension(client): Extension<UniOneClient>) -> Result<(), HandlerError> {
    let message = MessageObject {
        recipients: vec![Recipient::from_address("Alex <alex@example.com>", None)],
        from_email: Some("shop@example.com".into()),
        from_name: Some("Shop".into()),
        subject: "Your order".into(),
        body: BodyObject::html("<p>Hello, {{to_name}}! Your order has shipped.</p>"),
        ..Default::default()
    };
    client.email().send(&EmailSendRequest { message }).await?;
    Ok(())
}
```

## No domain yet: the test sandbox domain

Until your domain is verified, send from the **sandbox domain**: a test domain every account has, already verified, with DKIM and SPF set. It delivers only to addresses you confirm (the confirmation link comes by email, valid 48 hours), up to 100 emails a day. The sandbox domain is deleted 31 days after it was created, but you can get it again: the next `domain/get-sandbox-domain` call creates a new one (with a new name, so read it from the call, do not hardcode it).

The examples use the API client of the package (see the configuration above).

```rust
use axum::Extension;
use unione::api::confirmed_email::model::ConfirmedEmailRequest;
use unione::api::email::model::EmailSendRequest;
use unione::api::email::model::MessageObject;
use unione::api::email::model::MessageObjectRecipientsItem as Recipient;
use unione::common::model::BodyObject;
use unione::UniOneClient;
use unione_wrapper::HandlerError;

async fn try_sandbox(Extension(client): Extension<UniOneClient>) -> Result<(), HandlerError> {
    // 1. Your sandbox domain: created on the first call, the same one afterwards
    let sandbox = client.domain().get_sandbox_domain().await?;
    let domain = sandbox.domain.unwrap_or_default();

    // 2. Confirm your inbox: UniOne sends a link, click it within 48 hours
    let confirm = ConfirmedEmailRequest {
        email: "you@example.com".into(),
    };
    client.confirmed_email().create(&confirm).await?;

    // 3. Send from any address on the sandbox domain to the confirmed one
    let message = MessageObject {
        recipients: vec![Recipient::from_address("you@example.com", None)],
        from_email: Some(format!("noreply@{domain}")),
        subject: "Hello from UniOne".into(),
        body: BodyObject::html("<p>It works!</p>"),
        ..Default::default()
    };
    client.email().send(&EmailSendRequest { message }).await?;
    Ok(())
}
```

<details>
<summary>Sandbox limits</summary>

- Sends only to confirmed addresses: up to 10 per account.
- Up to 100 emails a day.
- The sandbox domain expires after 31 days; call `domain/get-sandbox-domain` again to get a new one.
- Works only with the account API key, not a project key.
- Not listed in `domain/list`; the other `domain/*` methods do not apply to it.

The sandbox is for development and testing. Do not use a sandbox domain as the sender in production.

</details>

<details>
<summary>The same with curl</summary>

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

</details>

## Going to production

**1. Verify your sending domain.** `domain/get-dns-records` returns the verification record, the DKIM key and the DMARC target; add the records at your DNS provider, then `domain/validate-verification-record` and `domain/validate-dkim`.

| Type | Host | Value | Where it comes from |
|---|---|---|---|
| TXT | `@` | `unione-validate-hash=…` | `verification-record` of the response, as is |
| TXT | `<selector>._domainkey` (the selector is on the domain page of the control panel, e.g. `us`) | `v=DKIM1; k=rsa; p=<dkim>` | `dkim` of the response after `v=DKIM1; k=rsa; p=` |
| TXT | `@` | `v=spf1 include:spf.unione.io ~all` | not in the response; add the include to your SPF record if you have one |
| CNAME | `_dmarc` | `<domain>.dmarc.…` | `dmarc` of the response (UniOne publishes the DMARC policy); keep your own DMARC TXT record instead if the domain has one |

**2. Make sending reliable.**

| | |
|---|---|
| Retries | `429` and `5xx` with backoff and the same `idempotence_key`; fix other errors instead of retrying |
| Limits | 500 recipients and 10 MB per request, attachments included: base64 makes a file about a third larger |
| Body | always `html` or `plaintext` (a template can supply it) |
| Delivery status | a webhook, not polling |
| Queue | send from a background job, not inside the web request |
| Config | `from_email` from config, so the sandbox → production switch needs no code change |
| Defaults | keep the unsubscribe footer and tracking; changing them needs UniOne support |

## Help

- [Integration guides](https://docs.unione.io/en/integrations)
- [Rust SDK](https://github.com/unione-repo/unione-rust-sdk)
- AI coding agents: [llms.txt](https://docs.unione.io/llms.txt), the [MCP server](https://unione.io/en/agents), skills: `npx skills add https://github.com/unione-repo/unione-skills.git`
- [Support](https://cp.unione.io/en/support) · [Issues](https://github.com/unione-repo/unione-axum/issues)
- Contributing: [AGENTS.md](AGENTS.md) — the package is generated from the OpenAPI spec.
