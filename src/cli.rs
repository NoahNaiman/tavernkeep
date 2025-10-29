//! Command-line parsing and the terminal client subcommands.

use crate::remote::{Remote, RemoteError};
use crate::term::{self, Style};

pub const DEFAULT_URL: &str = "http://127.0.0.1:8080";

pub const USAGE: &str = "\
usage:
  tavernkeep [serve]                        run the API (configured by TK_* env vars)
  tavernkeep warm --file <party.txt>        prefetch slugs into a running server

Readable terminal output from a running server:
  tavernkeep spell <name>                   e.g. tavernkeep spell fireball
  tavernkeep monster <name>                 e.g. tavernkeep monster adult red dragon
  tavernkeep search <query> [--kind spell|monster]
  tavernkeep roll <expr> [--seed N]         e.g. tavernkeep roll adv+3
  tavernkeep attack <monster> [action] [--seed N]
                                            action index; default: first attack

Client options:
  --url <http://host:port>                  server (default: $TK_URL, then
                                            $TK_WARM_URL, then http://127.0.0.1:8080)
  --json                                    print the raw JSON instead
  --no-color                                plain text (also honors NO_COLOR)
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Spell(String),
    Monster(String),
    Search {
        q: String,
        kind: Option<String>,
    },
    Roll {
        expr: String,
        seed: Option<u64>,
    },
    Attack {
        monster: String,
        action: Option<usize>,
        seed: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOpts {
    pub url: String,
    pub json: bool,
    pub no_color: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    Help,
    Warm { file: String, url: String },
    Client { query: Query, opts: ClientOpts },
}

/// Turn free-form words into a slug: `Adult Red Dragon` → `adult-red-dragon`.
pub fn slugify(words: &[String]) -> String {
    words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse arguments (without the program name). `env` looks up environment
/// variables so tests can control defaults.
pub fn parse(args: &[String], env: impl Fn(&str) -> Option<String>) -> Result<Command, String> {
    let default_url = env("TK_URL")
        .or_else(|| env("TK_WARM_URL"))
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    let Some(cmd) = args.first().map(String::as_str) else {
        return Ok(Command::Serve);
    };
    let rest = &args[1..];
    match cmd {
        "serve" if rest.is_empty() => Ok(Command::Serve),
        "serve" => Err("serve takes no arguments; configure it with TK_* env vars".into()),
        "-h" | "--help" | "help" => Ok(Command::Help),
        "warm" => {
            let mut file = None;
            let mut url = default_url;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                match flag.as_str() {
                    "--file" | "-f" => file = Some(value.clone()),
                    "--url" => url = value.clone(),
                    other => return Err(format!("unknown flag {other}")),
                }
            }
            let file = file.ok_or("warm needs --file <party.txt>")?;
            Ok(Command::Warm { file, url })
        }
        "spell" | "monster" | "search" | "roll" | "attack" => {
            let mut opts = ClientOpts {
                url: default_url,
                json: false,
                no_color: false,
            };
            let mut words = Vec::new();
            let mut seed = None;
            let mut kind = None;
            let mut it = rest.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--url" => opts.url = it.next().ok_or("--url needs a value")?.clone(),
                    "--json" => opts.json = true,
                    "--no-color" => opts.no_color = true,
                    "--seed" if matches!(cmd, "roll" | "attack") => {
                        let v = it.next().ok_or("--seed needs a value")?;
                        seed = Some(
                            v.parse()
                                .map_err(|_| format!("--seed must be a number, got {v:?}"))?,
                        );
                    }
                    "--kind" if cmd == "search" => {
                        let v = it.next().ok_or("--kind needs a value")?;
                        if v != "spell" && v != "monster" {
                            return Err(format!("--kind must be spell or monster, got {v:?}"));
                        }
                        kind = Some(v.clone());
                    }
                    // A leading '-' is a flag unless it looks like dice (`-1d4`).
                    flag if flag.starts_with("--") => {
                        return Err(format!("unknown option {flag} for {cmd}"));
                    }
                    word => words.push(word.to_string()),
                }
            }
            if words.is_empty() {
                return Err(format!("{cmd} needs an argument"));
            }
            let query = match cmd {
                "spell" => Query::Spell(slugify(&words)),
                "monster" => Query::Monster(slugify(&words)),
                "search" => Query::Search {
                    q: words.join(" "),
                    kind,
                },
                "roll" => Query::Roll {
                    expr: words.join(" "),
                    seed,
                },
                _ => {
                    // A trailing number is the action index.
                    let action = match words.last().map(|w| w.parse::<usize>()) {
                        Some(Ok(i)) if words.len() > 1 => {
                            words.pop();
                            Some(i)
                        }
                        _ => None,
                    };
                    Query::Attack {
                        monster: slugify(&words),
                        action,
                        seed,
                    }
                }
            };
            Ok(Command::Client { query, opts })
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

/// Run a client query; returns the text to print and the exit code.
pub async fn run(query: &Query, opts: &ClientOpts, style: &Style) -> (String, i32) {
    let remote = Remote::new(&opts.url);
    match render(&remote, query, opts.json, style).await {
        Ok(text) => (text, 0),
        Err(e) => {
            let expr = match query {
                Query::Roll { expr, .. } => Some(expr.as_str()),
                _ => None,
            };
            let mut text = describe_error(&e, expr, style);
            if let (RemoteError::Server { status: 404, .. }, Some((kind, slug))) =
                (&e, lookup_target(query))
            {
                text.push_str(&format!(
                    "\n  try: tavernkeep search {} --kind {kind}",
                    slug.replace('-', " ")
                ));
            }
            (text, 1)
        }
    }
}

/// The kind and slug a spell/monster lookup asked for.
fn lookup_target(query: &Query) -> Option<(&'static str, &str)> {
    match query {
        Query::Spell(s) => Some(("spell", s)),
        Query::Monster(m)
        | Query::Attack {
            monster: m,
            action: None,
            ..
        } => Some(("monster", m)),
        _ => None,
    }
}

fn describe_error(e: &RemoteError, expr: Option<&str>, style: &Style) -> String {
    match e {
        RemoteError::Server {
            message,
            position: Some(pos),
            ..
        } if expr.is_some() => term::error(message, expr.map(|x| (x.trim(), *pos)), style),
        RemoteError::Server {
            status: 503,
            message,
            retry_after,
            ..
        } => {
            let hint = retry_after
                .map(|s| format!(" (try again in {s}s)"))
                .unwrap_or_default();
            term::error(&format!("{message}{hint}"), None, style)
        }
        other => term::error(&other.to_string(), None, style),
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).expect("models serialize")
}

async fn render(
    remote: &Remote,
    query: &Query,
    json: bool,
    style: &Style,
) -> Result<String, RemoteError> {
    Ok(match query {
        Query::Spell(slug) => {
            let s = remote.spell(slug).await?;
            if json {
                to_json(&s)
            } else {
                term::spell(&s, style)
            }
        }
        Query::Monster(slug) => {
            let m = remote.monster(slug).await?;
            if json {
                to_json(&m)
            } else {
                term::monster(&m, style)
            }
        }
        Query::Search { q, kind } => {
            let kinds: Vec<&str> = match kind {
                Some(k) => vec![k.as_str()],
                None => vec!["spell", "monster"],
            };
            let mut parts = Vec::new();
            for k in kinds {
                let r = remote.search(q, k).await?;
                parts.push(if json {
                    to_json(&r)
                } else {
                    term::search(&r, style)
                });
            }
            parts.join("\n\n")
        }
        Query::Roll { expr, seed } => {
            let r = remote.roll(expr, *seed).await?;
            if json {
                to_json(&r)
            } else {
                term::roll(&r, style)
            }
        }
        Query::Attack {
            monster,
            action,
            seed,
        } => {
            let m = remote.monster(monster).await?;
            let index = match action {
                Some(i) => *i,
                None => m
                    .actions
                    .iter()
                    .position(|a| a.attack_bonus.is_some_and(|b| b != 0) && a.damage_dice.is_some())
                    .ok_or_else(|| RemoteError::Server {
                        status: 422,
                        message: format!("{} has no attack actions", m.name),
                        position: None,
                        retry_after: None,
                    })?,
            };
            let a = remote.attack(monster, index, *seed).await?;
            if json {
                to_json(&a)
            } else {
                term::attack(&a, &m.name, style)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn p(s: &str) -> Result<Command, String> {
        parse(&args(s), |_| None)
    }

    fn client(s: &str) -> Query {
        match p(s).unwrap() {
            Command::Client { query, .. } => query,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn server_commands() {
        assert_eq!(p("").unwrap(), Command::Serve);
        assert_eq!(p("serve").unwrap(), Command::Serve);
        assert!(p("serve now").is_err());
        assert_eq!(p("--help").unwrap(), Command::Help);
        assert_eq!(
            p("warm --file party.txt").unwrap(),
            Command::Warm {
                file: "party.txt".into(),
                url: DEFAULT_URL.into()
            }
        );
    }

    #[test]
    fn names_become_slugs() {
        assert_eq!(client("spell Fireball"), Query::Spell("fireball".into()));
        assert_eq!(
            client("monster Adult Red Dragon"),
            Query::Monster("adult-red-dragon".into())
        );
    }

    #[test]
    fn roll_joins_words_and_takes_a_seed() {
        assert_eq!(
            client("roll 2d6 + 3 --seed 9"),
            Query::Roll {
                expr: "2d6 + 3".into(),
                seed: Some(9)
            }
        );
        assert_eq!(
            client("roll -1d4+5"),
            Query::Roll {
                expr: "-1d4+5".into(),
                seed: None
            }
        );
        assert!(p("roll adv --seed nope").is_err());
        assert!(p("roll").is_err());
    }

    #[test]
    fn attack_takes_an_optional_trailing_index() {
        assert_eq!(
            client("attack adult red dragon 2 --seed 1"),
            Query::Attack {
                monster: "adult-red-dragon".into(),
                action: Some(2),
                seed: Some(1)
            }
        );
        assert_eq!(
            client("attack goblin"),
            Query::Attack {
                monster: "goblin".into(),
                action: None,
                seed: None
            }
        );
    }

    #[test]
    fn search_kind_is_validated() {
        assert_eq!(
            client("search fire bolt --kind spell"),
            Query::Search {
                q: "fire bolt".into(),
                kind: Some("spell".into())
            }
        );
        assert!(p("search fire --kind item").is_err());
        assert!(p("spell fireball --seed 3").is_err());
    }

    #[test]
    fn url_comes_from_flag_then_env() {
        let env = |k: &str| (k == "TK_URL").then(|| "http://table:9000".to_string());
        match parse(&args("spell fireball"), env).unwrap() {
            Command::Client { opts, .. } => assert_eq!(opts.url, "http://table:9000"),
            other => panic!("{other:?}"),
        }
        match parse(
            &args("spell fireball --url http://x:1 --json --no-color"),
            env,
        )
        .unwrap()
        {
            Command::Client { opts, .. } => {
                assert_eq!(opts.url, "http://x:1");
                assert!(opts.json && opts.no_color);
            }
            other => panic!("{other:?}"),
        }
    }
}
