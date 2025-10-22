//! `GET /monsters/{slug}/attack/{action_index}` with fixed seeds against fixture monsters.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use wiremock::MockServer;

async fn app() -> (MockServer, TestApp) {
    let server = MockServer::start().await;
    mount_fixtures(&server).await;
    let app = TestApp::new(test_config(&server.uri()));
    (server, app)
}

#[tokio::test]
async fn goblin_scimitar_with_fixed_seed() {
    let (_s, app) = app().await;
    let r = app.get("/monsters/goblin/attack/0?seed=7").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json,
        json!({
            "monster": "goblin",
            "action": "Scimitar",
            "to_hit": {"natural": 9, "bonus": 4, "total": 13},
            "critical": false,
            "damage": {
                "expr": "1d6+2",
                "total": 3,
                "rolls": [{"die": "1d6", "results": [1], "kept": [1]}],
                "modifier": 2
            }
        })
    );
    // Same seed, same roll.
    assert_eq!(
        app.get("/monsters/goblin/attack/0?seed=7").await.json,
        r.json
    );
}

#[tokio::test]
async fn natural_20_doubles_damage_dice_but_not_the_bonus() {
    let (_s, app) = app().await;
    let r = app.get("/monsters/goblin/attack/0?seed=27").await;
    assert_eq!(r.json["to_hit"]["natural"], 20);
    assert_eq!(r.json["critical"], true);
    assert_eq!(r.json["damage"]["expr"], "2d6+2");
    assert_eq!(r.json["damage"]["modifier"], 2);
    assert_eq!(r.json["damage"]["rolls"][0]["results"], json!([2, 2]));
    assert_eq!(r.json["damage"]["total"], 6);

    let r = app.get("/monsters/adult-red-dragon/attack/1?seed=27").await;
    assert_eq!(r.json["action"], "Bite");
    assert_eq!(r.json["critical"], true);
    assert_eq!(r.json["damage"]["expr"], "4d10+4d6+8");
    assert_eq!(r.json["damage"]["total"], 42);
}

#[tokio::test]
async fn attack_totals_are_consistent_across_seeds() {
    let (_s, app) = app().await;
    for seed in 0..40 {
        let r = app
            .get(&format!("/monsters/orc/attack/0?seed={seed}"))
            .await;
        assert_eq!(r.status, StatusCode::OK);
        let natural = r.json["to_hit"]["natural"].as_i64().unwrap();
        assert!((1..=20).contains(&natural));
        assert_eq!(r.json["to_hit"]["total"].as_i64().unwrap(), natural + 5);
        assert_eq!(r.json["critical"], natural == 20);
        let kept: i64 = r.json["damage"]["rolls"][0]["kept"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .sum();
        assert_eq!(r.json["damage"]["total"].as_i64().unwrap(), kept + 3);
    }
}

#[tokio::test]
async fn attack_errors() {
    let (_s, app) = app().await;
    // Multiattack has no attack bonus.
    let r = app.get("/monsters/adult-red-dragon/attack/0?seed=1").await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.json["message"].as_str().unwrap().contains("Multiattack"));
    // Out of range.
    let r = app.get("/monsters/goblin/attack/2").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Not a number.
    let r = app.get("/monsters/goblin/attack/first").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Unknown monster.
    let r = app.get("/monsters/not-a-monster/attack/0").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}
