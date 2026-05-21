//! Integration tests for the HTTP sync leg, exercised against a local
//! `wiremock` mock server.
//!
//! `sync::post_update` uses a *blocking* reqwest client, which must not run on
//! a Tokio worker thread (it spins up its own runtime). We therefore run the
//! call on `spawn_blocking`, which executes on a dedicated blocking thread.

use foxlogi_stockpiler_lib::sync::{self, SyncError, UpdatePayload};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sample_payload() -> UpdatePayload {
    UpdatePayload {
        filename: "76561198012791572_MapData.sav".into(),
        modified_at: "2026-05-20T14:30:00Z".into(),
        data: json!([{ "MapId": "EWorldConquestMapId::HowlCountyHex" }]),
    }
}

#[tokio::test]
async fn posts_payload_to_endpoint_with_bearer_token_and_exact_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(sync::STOCKPILE_PATH))
        .and(header("authorization", "Bearer secret-key-123"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "filename": "76561198012791572_MapData.sav",
            "modified_at": "2026-05-20T14:30:00Z",
            "data": [{ "MapId": "EWorldConquestMapId::HowlCountyHex" }]
        })))
        .respond_with(ResponseTemplate::new(200))
        .expect(1) // verified on server drop
        .mount(&server)
        .await;

    let base = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = reqwest::blocking::Client::new();
        sync::post_update(&client, &base, "secret-key-123", &sample_payload())
    })
    .await
    .expect("blocking task panicked");

    assert!(result.is_ok(), "expected success, got {result:?}");
}

#[tokio::test]
async fn server_error_is_reported_with_status_and_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(sync::STOCKPILE_PATH))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
        .mount(&server)
        .await;

    let base = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = reqwest::blocking::Client::new();
        sync::post_update(&client, &base, "wrong-key", &sample_payload())
    })
    .await
    .expect("blocking task panicked");

    match result {
        Err(SyncError::Status { status, body }) => {
            assert_eq!(status, 401);
            assert!(body.contains("invalid api key"), "body was: {body}");
        }
        other => panic!("expected SyncError::Status, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_api_key_short_circuits_before_any_request() {
    // No mock mounted: if a request were sent, the server would return 404 and
    // the test would fail with a different error.
    let server = MockServer::start().await;
    let base = server.uri();

    let result = tokio::task::spawn_blocking(move || {
        let client = reqwest::blocking::Client::new();
        sync::post_update(&client, &base, "   ", &sample_payload())
    })
    .await
    .expect("blocking task panicked");

    assert!(matches!(result, Err(SyncError::MissingApiKey)));
}
