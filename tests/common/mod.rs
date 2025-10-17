//! Shared helpers for integration tests: fixtures, a wiremock upstream and
//! an in-process app driven through `tower::ServiceExt::oneshot`.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::Value;
use tavernkeep::api::AppState;
use tavernkeep::clock::{MockClock, SharedClock};
use tavernkeep::config::Config;
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const SPELLS: [&str; 5] = ["fireball", "magic-missile", "cure-wounds", "shield", "wish"];
pub const MONSTERS: [&str; 5] = ["goblin", "adult-red-dragon", "orc", "zombie", "lich"];

pub fn fixture(rel: &str) -> Value {
    let p = format!("{}/tests/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .unwrap()
}

/// Config pointed at `upstream` with fast retries for tests.
pub fn test_config(upstream: &str) -> Config {
    Config {
        upstream_url: upstream.to_string(),
        upstream_timeout: Duration::from_millis(500),
        backoff_base: Duration::from_millis(1),
        backoff_max: Duration::from_millis(5),
        ..Config::default()
    }
}

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub clock: Arc<MockClock>,
}

impl TestApp {
    pub fn new(config: Config) -> Self {
        let clock = MockClock::new();
        let shared: SharedClock = clock.clone();
        let (router, state) = tavernkeep::app(config, shared);
        TestApp {
            router,
            state,
            clock,
        }
    }

    pub async fn get(&self, uri: &str) -> Resp {
        self.request(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    pub async fn request(&self, req: Request<Body>) -> Resp {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            json,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub json: Value,
    pub text: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// Mount every recorded fixture on `server`.
pub async fn mount_fixtures(server: &MockServer) {
    for s in SPELLS {
        Mock::given(method("GET"))
            .and(path(format!("/spells/{s}/")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture(&format!("spells/{s}.json"))),
            )
            .mount(server)
            .await;
    }
    for m in MONSTERS {
        Mock::given(method("GET"))
            .and(path(format!("/monsters/{m}/")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture(&format!("monsters/{m}.json"))),
            )
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/documents/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("documents.json")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/spells/"))
        .and(query_param("search", "fire"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("search/spells-fire.json")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/monsters/"))
        .and(query_param("search", "dragon"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("search/monsters-dragon.json")),
        )
        .mount(server)
        .await;
}

/// Number of requests `server` received for exactly `p`.
pub async fn hits(server: &MockServer, p: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == p)
        .count()
}
