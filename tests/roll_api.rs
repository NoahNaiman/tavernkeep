//! HTTP tests for `POST /roll`.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn post_roll(body: &str) -> (StatusCode, Value) {
    let resp = tavernkeep::api::router()
        .oneshot(
            Request::post("/roll")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn rolls_with_seed_are_reproducible() {
    let req = json!({"expr": "2d20kh1+5", "seed": 1234}).to_string();
    let (s1, a) = post_roll(&req).await;
    let (s2, b) = post_roll(&req).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(a, b);
    assert_eq!(a["expr"], "2d20kh1+5");
    assert_eq!(a["modifier"], 5);
    assert_eq!(a["rolls"][0]["die"], "2d20kh1");
    assert_eq!(a["rolls"][0]["results"].as_array().unwrap().len(), 2);
    assert_eq!(a["rolls"][0]["kept"].as_array().unwrap().len(), 1);
    let kept = a["rolls"][0]["kept"][0].as_i64().unwrap();
    assert_eq!(a["total"].as_i64().unwrap(), kept + 5);
}

#[tokio::test]
async fn over_limit_returns_422_with_position() {
    let (status, body) = post_roll(r#"{"expr": "1d6+101d6"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["position"], 4);
    assert!(body["message"].as_str().unwrap().contains("position 4"));
}

#[tokio::test]
async fn syntax_error_returns_422_with_position() {
    let (status, body) = post_roll(r#"{"expr": "2d6 * 2"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["position"], 4);
}

#[tokio::test]
async fn malformed_json_returns_400() {
    let (status, body) = post_roll(r#"{"exp": "1d6"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");
}
