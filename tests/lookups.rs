//! Lookup, search, health and metrics endpoints against a mock Open5e.

mod common;

use axum::http::StatusCode;
use common::*;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn app_with_fixtures() -> (MockServer, TestApp) {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));
    (server, app)
}

#[tokio::test]
async fn spell_lookup_returns_internal_model() {
    let (_s, app) = app_with_fixtures().await;
    let r = app.get("/spells/fireball").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["slug"], "fireball");
    assert_eq!(r.json["level"], 3);
    assert_eq!(r.json["school"], "Evocation");
    assert_eq!(r.json["concentration"], false);
    assert!(
        r.json.get("document__slug").is_none(),
        "unknown fields leak"
    );
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert!(r.header("cache-control").unwrap().contains("max-age=3600"));
}

#[tokio::test]
async fn monster_lookup_returns_internal_model() {
    let (_s, app) = app_with_fixtures().await;
    let r = app.get("/monsters/adult-red-dragon").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["type"], "Dragon");
    assert_eq!(r.json["cr"], 17.0);
    assert_eq!(r.json["speed"]["fly"], 80);
    assert_eq!(r.json["abilities"]["str"], 27);
    assert_eq!(r.json["actions"][1]["name"], "Bite");
    assert_eq!(r.json["actions"][1]["damage_dice"], "2d10+2d6");
}

#[tokio::test]
async fn unknown_slug_is_404() {
    let (_s, app) = app_with_fixtures().await;
    let r = app.get("/spells/not-a-spell").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json["error"], "not_found");
}

#[tokio::test]
async fn invalid_slug_is_400_without_upstream_call() {
    let (s, app) = app_with_fixtures().await;
    let r = app.get("/spells/Fire%2Fball").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(s.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn search_proxies_open5e() {
    let (_s, app) = app_with_fixtures().await;
    let r = app.get("/search?q=fire&kind=spell").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["kind"], "spell");
    assert_eq!(r.json["query"], "fire");
    assert_eq!(r.json["results"].as_array().unwrap().len(), 5);
    assert_eq!(r.json["results"][0]["slug"], "alchemical-form");
    assert!(r.json["count"].as_u64().unwrap() >= 5);
    assert!(r.header("cache-control").unwrap().contains("max-age=300"));

    let r = app.get("/search?q=dragon&kind=monster").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["kind"], "monster");
}

#[tokio::test]
async fn search_validates_query_and_kind() {
    let (s, app) = app_with_fixtures().await;
    for uri in [
        "/search?q=f&kind=spell",
        "/search?kind=spell",
        &format!("/search?q={}&kind=spell", "x".repeat(65)),
        "/search?q=fire&kind=item",
        "/search?q=fire",
    ] {
        let r = app.get(uri).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}");
    }
    assert!(s.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn upstream_failure_without_cache_is_502() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let app = TestApp::new(test_config(&server.uri()));
    let r = app.get("/spells/fireball").await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn healthz_is_always_ok() {
    let app = TestApp::new(test_config("http://127.0.0.1:9"));
    let r = app.get("/healthz").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["status"], "ok");
}

#[tokio::test]
async fn readyz_tracks_recent_upstream_contact() {
    let (_s, app) = app_with_fixtures().await;
    assert_eq!(
        app.get("/readyz").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(app.get("/spells/fireball").await.status, StatusCode::OK);
    let r = app.get("/readyz").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["upstream_reachable"], true);
}

#[tokio::test]
async fn metrics_expose_request_counters() {
    let (_s, app) = app_with_fixtures().await;
    app.get("/spells/fireball").await;
    let r = app.get("/metrics").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text.contains("tk_requests_total"), "{}", r.text);
    assert!(r.text.contains("route=\"/spells/{slug}\""), "{}", r.text);
    assert!(r.text.contains("tk_upstream_requests_total"), "{}", r.text);
    assert!(
        r.text.contains("tk_upstream_duration_seconds_bucket"),
        "{}",
        r.text
    );
}
