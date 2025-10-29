//! Terminal client subcommands against a real in-process server.

mod common;

use common::*;
use tavernkeep::cli::{self, ClientOpts, Command, Query};
use tavernkeep::term::Style;
use wiremock::MockServer;

const PLAIN: Style = Style {
    color: false,
    width: 72,
};

async fn server() -> (MockServer, String) {
    let upstream = MockServer::start().await;
    mount_fixtures(&upstream).await;
    let app = TestApp::new(test_config(&upstream.uri()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.router).await.unwrap() });
    (upstream, format!("http://{addr}"))
}

async fn run(url: &str, line: &str) -> (String, i32) {
    let args: Vec<String> = line.split_whitespace().map(String::from).collect();
    let Command::Client { query, mut opts } = cli::parse(&args, |_| None).unwrap() else {
        panic!("not a client command: {line}");
    };
    opts.url = url.to_string();
    cli::run(&query, &opts, &PLAIN).await
}

#[tokio::test]
async fn spell_and_monster_render_readably() {
    let (_u, url) = server().await;
    let (out, code) = run(&url, "spell Fireball").await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Fireball\n3rd-level evocation\n"), "{out}");
    assert!(out.contains("Range         150 feet"), "{out}");

    let (out, code) = run(&url, "monster adult red dragon").await;
    assert_eq!(code, 0);
    assert!(
        out.starts_with("Adult Red Dragon\nHuge dragon, CR 17"),
        "{out}"
    );
    assert!(out.contains("27 (+8)"), "{out}");
}

#[tokio::test]
async fn seeded_roll_and_attack() {
    let (_u, url) = server().await;
    let (out, code) = run(&url, "roll 4d6kh3+2 --seed 42").await;
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "4d6kh3+2  →  12\n  4d6kh3    [(1), 4, 2, 4]  = 10\n  modifier  +2"
    );

    // No index: the first real attack (the dragon's index 0 is Multiattack).
    let (out, code) = run(&url, "attack adult red dragon --seed 27").await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Adult Red Dragon: Bite\n"), "{out}");
    assert!(out.contains("critical hit"), "{out}");

    let (out, _) = run(&url, "attack goblin 0 --seed 7").await;
    assert_eq!(
        out,
        "Goblin: Scimitar\nTo hit   d20 9 +4 = 13\nDamage   1d6+2 [1] +2 = 3"
    );
}

#[tokio::test]
async fn search_with_kind_lists_matches() {
    let (_u, url) = server().await;
    let (out, code) = run(&url, "search fire --kind spell").await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Spell results for \"fire\""), "{out}");
}

#[tokio::test]
async fn errors_are_readable() {
    let (_u, url) = server().await;
    let (out, code) = run(&url, "roll 2d20kh1+101d6").await;
    assert_eq!(code, 1);
    assert_eq!(
        out,
        "error: invalid dice expression\n  2d20kh1+101d6\n          ^ dice count must be between 1 and 100"
    );

    let (out, code) = run(&url, "monster not a monster").await;
    assert_eq!(code, 1);
    assert_eq!(
        out,
        "error: no monster named \"not-a-monster\"\n  try: tavernkeep search not a monster --kind monster"
    );

    let (out, code) = run(&url, "attack goblin 9").await;
    assert_eq!(code, 1);
    assert!(out.starts_with("error: Goblin has 2 actions"), "{out}");
}

#[tokio::test]
async fn unreachable_server_says_so() {
    let query = Query::Spell("fireball".into());
    let opts = ClientOpts {
        url: "http://127.0.0.1:9".into(),
        json: false,
        no_color: true,
    };
    let (out, code) = cli::run(&query, &opts, &PLAIN).await;
    assert_eq!(code, 1);
    assert!(out.contains("is `tavernkeep serve` running?"), "{out}");
}

#[tokio::test]
async fn json_flag_prints_raw_model() {
    let (_u, url) = server().await;
    let (out, code) = run(&url, "spell fireball --json").await;
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["slug"], "fireball");
}
