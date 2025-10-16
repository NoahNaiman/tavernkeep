//! Cache, coalescing, retry and breaker behavior against a mock Open5e.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use tavernkeep::breaker::BreakerState;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HOUR: Duration = Duration::from_secs(3600);

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn fail_everything(server: &MockServer) {
    server.reset().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .mount(server)
        .await;
}

fn all_slugs() -> Vec<String> {
    SPELLS
        .iter()
        .map(|s| format!("/spells/{s}"))
        .chain(MONSTERS.iter().map(|m| format!("/monsters/{m}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn coalescing_200_concurrent_cold_requests_make_one_upstream_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spells/fireball/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture("spells/fireball.json"))
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let app = Arc::new(TestApp::new(test_config(&server.uri())));

    let tasks: Vec<_> = (0..200)
        .map(|_| {
            let app = app.clone();
            tokio::spawn(async move { app.get("/spells/fireball").await })
        })
        .collect();
    for t in tasks {
        let r = t.await.unwrap();
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json["name"], "Fireball");
    }
    assert_eq!(hits(&server, "/spells/fireball/").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn outage_never_returns_5xx_for_cached_slugs_and_recovers() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = Arc::new(TestApp::new(test_config(&server.uri())));

    // Warm 10 slugs.
    for uri in all_slugs() {
        assert_eq!(app.get(&uri).await.status, StatusCode::OK, "{uri}");
    }

    // Upstream goes down; cached copies age past the fresh TTL so every hit
    // is stale and triggers a background refresh that fails.
    fail_everything(&server).await;
    app.clock.advance(2 * HOUR);

    let slugs = Arc::new(all_slugs());
    let tasks: Vec<_> = (0..2000)
        .map(|i| {
            let app = app.clone();
            let slugs = slugs.clone();
            tokio::spawn(async move { app.get(&slugs[i % slugs.len()]).await.status })
        })
        .collect();
    let mut server_errors = 0;
    for t in tasks {
        let status = t.await.unwrap();
        if status.is_server_error() {
            server_errors += 1;
        }
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(server_errors, 0);

    let state = app.state.clone();
    wait_for("breaker to open", || {
        matches!(state.service.breaker_state(), BreakerState::Open { .. })
    })
    .await;
    assert!(
        server.received_requests().await.unwrap().len() >= 5,
        "refreshes reached upstream"
    );

    // Upstream recovers with changed data; after the cooldown the next stale
    // hit sends the half-open probe, which succeeds and closes the breaker.
    server.reset().await;
    let mut revised = fixture("spells/fireball.json");
    revised["name"] = "Fireball (revised)".into();
    Mock::given(method("GET"))
        .and(path("/spells/fireball/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(revised))
        .mount(&server)
        .await;
    mount_fixtures(&server).await;
    app.clock.advance(Duration::from_secs(31));

    let r = app.get("/spells/fireball").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["name"], "Fireball", "stale copy served during probe");
    wait_for("breaker to close", || {
        state.service.breaker_state() == BreakerState::Closed { failures: 0 }
    })
    .await;
    let svc = state.service.clone();
    let mut fresh = false;
    for _ in 0..100 {
        let r = app.get("/spells/fireball").await;
        if r.json["name"] == "Fireball (revised)" {
            assert!(r.header("cache-control").unwrap().contains("max-age=3600"));
            fresh = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(fresh, "recovered data was never served");
    assert!(svc.upstream_recently_reachable());
}

#[tokio::test]
async fn never_cached_slug_during_outage_gets_503_with_retry_after() {
    let server = MockServer::start().await;
    fail_everything(&server).await;
    let app = TestApp::new(test_config(&server.uri()));

    // Five failed logical calls (each retried 3x) open the breaker.
    for i in 0..5 {
        let r = app.get(&format!("/monsters/missing-{i}")).await;
        assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    }
    assert_eq!(hits_total(&server).await, 15, "3 attempts per call");
    assert!(matches!(
        app.state.service.breaker_state(),
        BreakerState::Open { .. }
    ));

    let r = app.get("/monsters/goblin").await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.header("retry-after"), Some("30"));
    assert_eq!(r.json["error"], "unavailable");
    assert_eq!(hits_total(&server).await, 15, "breaker short-circuits");

    app.clock.advance(Duration::from_secs(10));
    let r = app.get("/monsters/goblin").await;
    assert_eq!(r.header("retry-after"), Some("20"));
}

async fn hits_total(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn etag_and_if_none_match_return_304() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));

    let first = app.get("/monsters/goblin").await;
    let etag = first.header("etag").unwrap().to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'));

    for value in [
        etag.clone(),
        format!("W/{etag}"),
        format!("\"nope\", {etag}"),
    ] {
        let r = app
            .request(
                Request::get("/monsters/goblin")
                    .header("if-none-match", value.as_str())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{value}");
        assert!(r.text.is_empty());
        assert_eq!(r.header("etag"), Some(etag.as_str()));
        assert!(r.header("cache-control").is_some());
    }

    let r = app
        .request(
            Request::get("/monsters/goblin")
                .header("if-none-match", "\"something-else\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["name"], "Goblin");
    assert_eq!(hits(&server, "/monsters/goblin/").await, 1);
}

#[tokio::test]
async fn upstream_404_is_cached_for_60_seconds() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));

    for _ in 0..3 {
        assert_eq!(
            app.get("/spells/no-such-spell").await.status,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(hits(&server, "/spells/no-such-spell/").await, 1);

    app.clock.advance(Duration::from_secs(59));
    assert_eq!(
        app.get("/spells/no-such-spell").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(hits(&server, "/spells/no-such-spell/").await, 1);

    app.clock.advance(Duration::from_secs(1));
    assert_eq!(
        app.get("/spells/no-such-spell").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(hits(&server, "/spells/no-such-spell/").await, 2);
}

#[tokio::test]
async fn stale_entries_are_served_with_zero_max_age() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));
    app.get("/spells/shield").await;
    fail_everything(&server).await;
    app.clock.advance(2 * HOUR);
    let r = app.get("/spells/shield").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("cache-control"), Some("public, max-age=0"));
}

#[tokio::test]
async fn retries_5xx_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(path("/spells/wish/"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/spells/wish/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("spells/wish.json")))
        .with_priority(2)
        .mount(&server)
        .await;
    let app = TestApp::new(test_config(&server.uri()));
    let r = app.get("/spells/wish").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(hits(&server, "/spells/wish/").await, 3);
    assert_eq!(
        app.state.service.breaker_state(),
        BreakerState::Closed { failures: 0 }
    );
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/spells/wish/"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let app = TestApp::new(test_config(&server.uri()));
    assert_eq!(
        app.get("/spells/wish").await.status,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(hits(&server, "/spells/wish/").await, 1);
    // 404 is a normal answer, not an error: one call, no retry.
    Mock::given(path("/spells/gone/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert_eq!(app.get("/spells/gone").await.status, StatusCode::NOT_FOUND);
    assert_eq!(hits(&server, "/spells/gone/").await, 1);
}

#[tokio::test]
async fn timeouts_are_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/spells/shield/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture("spells/shield.json"))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let mut config = test_config(&server.uri());
    config.upstream_timeout = Duration::from_millis(50);
    let app = TestApp::new(config);
    assert_eq!(
        app.get("/spells/shield").await.status,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(hits(&server, "/spells/shield/").await, 3);
}

#[tokio::test]
async fn connection_errors_are_retried_and_open_the_breaker() {
    // Port 9 (discard) on localhost refuses connections.
    let app = TestApp::new(test_config("http://127.0.0.1:9"));
    for _ in 0..5 {
        assert_eq!(
            app.get("/spells/fireball").await.status,
            StatusCode::BAD_GATEWAY
        );
    }
    assert!(matches!(
        app.state.service.breaker_state(),
        BreakerState::Open { .. }
    ));
}

#[tokio::test]
async fn readyz_stays_ready_from_cache_after_upstream_goes_quiet() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));
    app.get("/monsters/orc").await;
    app.clock.advance(Duration::from_secs(120));
    let r = app.get("/readyz").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["upstream_reachable"], false);
    assert_eq!(r.json["cached_entries"], 1);
}

#[tokio::test]
async fn search_is_cached_for_five_minutes() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));
    for _ in 0..3 {
        assert_eq!(
            app.get("/search?q=fire&kind=spell").await.status,
            StatusCode::OK
        );
    }
    assert_eq!(hits(&server, "/spells/").await, 1);
    app.clock.advance(Duration::from_secs(301));
    assert_eq!(
        app.get("/search?q=fire&kind=spell").await.status,
        StatusCode::OK
    );
    assert_eq!(hits(&server, "/spells/").await, 2);
}
