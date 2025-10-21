//! `tavernkeep warm --file party.txt`: prefetch slugs into a running server.
//!
//! The cache lives in the server process, so warming works by asking the
//! server for each entry; every lookup it serves lands in its cache.
//!
//! File format, one entry per line (blank lines and `#` comments ignored):
//!
//! ```text
//! spell fireball
//! monster goblin
//! ```

use std::time::Duration;

use crate::service::{Kind, validate_slug};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub slug: String,
}

impl Entry {
    fn path(&self) -> String {
        match self.kind {
            Kind::Spell => format!("/spells/{}", self.slug),
            Kind::Monster => format!("/monsters/{}", self.slug),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

/// Parse a party file.
pub fn parse_party(text: &str) -> Result<Vec<Entry>, ParseError> {
    let mut entries = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |message: String| ParseError {
            line: i + 1,
            message,
        };
        let mut parts = line.split_whitespace();
        let (Some(kind), Some(slug), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(err(format!(
                "expected `spell <slug>` or `monster <slug>`, got {line:?}"
            )));
        };
        let kind = Kind::parse(&kind.to_ascii_lowercase())
            .ok_or_else(|| err(format!("unknown kind {kind:?} (use spell or monster)")))?;
        validate_slug(slug).map_err(|e| err(e.to_string()))?;
        entries.push(Entry {
            kind,
            slug: slug.to_string(),
        });
    }
    Ok(entries)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub warmed: Vec<Entry>,
    pub not_found: Vec<Entry>,
    pub failed: Vec<(Entry, String)>,
}

impl Report {
    pub fn is_success(&self) -> bool {
        self.not_found.is_empty() && self.failed.is_empty()
    }
}

/// Request every entry from the server at `base_url`.
pub async fn run(base_url: &str, entries: &[Entry]) -> Report {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("static reqwest configuration is valid");
    let base = base_url.trim_end_matches('/');
    let mut report = Report::default();
    for entry in entries {
        let url = format!("{base}{}", entry.path());
        match http.get(&url).send().await {
            Ok(r) if r.status().is_success() => report.warmed.push(entry.clone()),
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                report.not_found.push(entry.clone())
            }
            Ok(r) => report
                .failed
                .push((entry.clone(), format!("HTTP {}", r.status().as_u16()))),
            Err(e) => report.failed.push((entry.clone(), e.to_string())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_comments_and_blanks() {
        let entries = parse_party("# party\nspell fireball\n\n  MONSTER goblin  # boss\n").unwrap();
        assert_eq!(
            entries,
            vec![
                Entry {
                    kind: Kind::Spell,
                    slug: "fireball".into()
                },
                Entry {
                    kind: Kind::Monster,
                    slug: "goblin".into()
                },
            ]
        );
    }

    #[test]
    fn reports_line_numbers() {
        assert_eq!(
            parse_party("spell fireball\nitem sword").unwrap_err().line,
            2
        );
        assert_eq!(parse_party("\n\nfireball").unwrap_err().line, 3);
        assert_eq!(parse_party("spell Fire/ball").unwrap_err().line, 1);
        assert_eq!(parse_party("spell a b").unwrap_err().line, 1);
    }
}
