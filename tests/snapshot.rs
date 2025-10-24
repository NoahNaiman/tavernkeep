//! Cache snapshots: a restarted instance pointed at a dead upstream still
//! serves what the previous instance had cached.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use tavernkeep::snapshot;
use wiremock::MockServer;

#[tokio::test]
async fn restart_against_dead_upstream_serves_snapshotted_entries() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let first = TestApp::new(test_config(&server.uri()));
    for uri in ["/spells/fireball", "/monsters/goblin", "/spells/not-real"] {
        first.get(uri).await;
    }
    first.clock.advance(Duration::from_secs(600));

    let path = std::env::temp_dir().join(format!("tk-restart-{}.json", std::process::id()));
    snapshot::save(&path, &first.state.service.snapshot()).unwrap();
    let snap = snapshot::load(&path).unwrap().unwrap();
    assert_eq!(snap.spells.len(), 1, "404s are not snapshotted");
    assert_eq!(snap.monsters.len(), 1);
    assert_eq!(snap.spells[0].age_ms, 600_000);

    // "Restart": a new instance on a dead upstream, restored 5 minutes later.
    let second = TestApp::new(test_config("http://127.0.0.1:9"));
    let restored = second
        .state
        .service
        .restore(snap, Duration::from_secs(300))
        .await;
    assert_eq!(restored, 2);

    let r = second.get("/spells/fireball").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["name"], "Fireball");
    // 3600s fresh TTL minus 600s before the save and 300s of downtime.
    assert_eq!(r.header("cache-control"), Some("public, max-age=2700"));
    assert_eq!(second.get("/monsters/goblin").await.status, StatusCode::OK);
    // Anything not in the snapshot still fails while upstream is dead.
    assert_eq!(
        second.get("/monsters/lich").await.status,
        StatusCode::BAD_GATEWAY
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn restored_entries_past_fresh_are_stale_and_past_stale_are_dropped() {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let first = TestApp::new(test_config(&server.uri()));
    first.get("/spells/shield").await;
    let snap = first.state.service.snapshot();

    let second = TestApp::new(test_config("http://127.0.0.1:9"));
    let restored = second
        .state
        .service
        .restore(snap.clone(), Duration::from_secs(2 * 3600))
        .await;
    assert_eq!(restored, 1);
    let r = second.get("/spells/shield").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("cache-control"), Some("public, max-age=0"));

    let third = TestApp::new(test_config("http://127.0.0.1:9"));
    let restored = third
        .state
        .service
        .restore(snap, Duration::from_secs(8 * 24 * 3600))
        .await;
    assert_eq!(restored, 0);
}
