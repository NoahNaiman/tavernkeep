//! `warm` against a real in-process server backed by a mock Open5e.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use tavernkeep::warm;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_prefetches_so_lookups_survive_an_outage() {
    let upstream = MockServer::start().await;
    mount_fixtures(&upstream).await;
    let app = TestApp::new(test_config(&upstream.uri()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let party = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/party.example.txt"))
        .unwrap()
        + "spell not-a-real-spell\n";
    let entries = warm::parse_party(&party).unwrap();
    assert_eq!(entries.len(), 11);

    let report = warm::run(&format!("http://{addr}"), &entries).await;
    assert_eq!(report.warmed.len(), 10);
    assert_eq!(report.not_found.len(), 1);
    assert!(report.failed.is_empty());
    assert!(!report.is_success());

    // Offline at the table: every warmed entry is still served, even stale.
    upstream.reset().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&upstream)
        .await;
    app.clock.advance(Duration::from_secs(3 * 3600));
    for e in &report.warmed {
        let uri = match e.kind.as_str() {
            "spell" => format!("/spells/{}", e.slug),
            _ => format!("/monsters/{}", e.slug),
        };
        assert_eq!(app.get(&uri).await.status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn warm_reports_an_unreachable_server() {
    let entries = warm::parse_party("spell fireball").unwrap();
    let report = warm::run("http://127.0.0.1:9", &entries).await;
    assert_eq!(report.failed.len(), 1);
    assert!(!report.is_success());
}
