//! Human-readable terminal rendering of Tavernkeep responses.
//!
//! Every function is pure: it takes a model and a [`Style`] and returns the
//! text to print, so output can be tested with color off.

use crate::api::AttackRoll;
use crate::dice::{DieRoll, RollOutcome};
use crate::open5e::{Monster, SearchResults, Spell};

/// ANSI styling, or plain text when color is off.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
    /// Wrap width for prose.
    pub width: usize,
}

impl Style {
    pub fn plain(width: usize) -> Self {
        Style {
            color: false,
            width,
        }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn red(&self, s: &str) -> String {
        self.paint("1;31", s)
    }
    fn green(&self, s: &str) -> String {
        self.paint("1;32", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    fn title(&self, s: &str) -> String {
        self.paint("1;33", s)
    }
    /// Dropped dice: dim + strikethrough with color, `(x)` without.
    fn dropped(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2;9m{s}\x1b[0m")
        } else {
            format!("({s})")
        }
    }
    fn rule(&self) -> String {
        self.dim(&"─".repeat(self.width.min(60)))
    }
}

/// Greedy word wrap with a hanging `indent`.
pub fn wrap(text: &str, width: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let avail = width.saturating_sub(indent).max(20);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = para.trim();
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > avail {
                out.push(format!("{pad}{line}"));
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push(format!("{pad}{line}"));
        }
    }
    out.join("\n")
}

fn ordinal(n: u8) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// `0.25` → `1/4`, `17.0` → `17`.
pub fn challenge_rating(cr: f64) -> String {
    for (value, text) in [(0.125, "1/8"), (0.25, "1/4"), (0.5, "1/2")] {
        if (cr - value).abs() < 1e-9 {
            return text.to_string();
        }
    }
    if cr.fract() == 0.0 {
        format!("{cr:.0}")
    } else {
        cr.to_string()
    }
}

/// Ability modifier, e.g. `14` → `+2`, `8` → `-1`.
pub fn modifier(score: u8) -> String {
    let m = (i32::from(score) - 10).div_euclid(2);
    if m >= 0 {
        format!("+{m}")
    } else {
        m.to_string()
    }
}

fn signed(n: i64) -> String {
    if n >= 0 {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

fn field(style: &Style, label: &str, value: &str) -> String {
    format!("{} {value}", style.bold(&format!("{label:<13}")))
}

pub fn spell(s: &Spell, style: &Style) -> String {
    let school = s.school.as_deref().unwrap_or("unknown school");
    let subtitle = match s.level {
        Some(0) => format!("{school} cantrip"),
        Some(n) => format!("{}-level {}", ordinal(n), school.to_lowercase()),
        None => school.to_string(),
    };
    let mut tags = Vec::new();
    if s.concentration == Some(true) {
        tags.push("concentration");
    }
    if s.ritual == Some(true) {
        tags.push("ritual");
    }
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!("  {}", style.cyan(&format!("[{}]", tags.join(", "))))
    };

    let mut out = vec![
        style.title(&s.name),
        format!("{}{tags}", style.dim(&subtitle)),
        style.rule(),
    ];
    let dash = "—".to_string();
    for (label, value) in [
        ("Casting Time", &s.casting_time),
        ("Range", &s.range),
        ("Components", &s.components),
        ("Duration", &s.duration),
    ] {
        out.push(field(style, label, value.as_ref().unwrap_or(&dash)));
    }
    if let Some(desc) = s.description.as_deref().filter(|d| !d.trim().is_empty()) {
        out.push(style.rule());
        out.push(wrap(desc, style.width, 0));
    }
    out.join("\n")
}

fn speed(m: &Monster) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(walk) = m.speed.get("walk") {
        parts.push(format!("{walk} ft."));
    }
    for (mode, feet) in &m.speed {
        if mode != "walk" {
            parts.push(format!("{mode} {feet} ft."));
        }
    }
    if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn monster(m: &Monster, style: &Style) -> String {
    let size = m.size.as_deref().unwrap_or("");
    let kind = m.kind.as_deref().unwrap_or("creature").to_lowercase();
    let cr =
        m.cr.map(|c| format!(", CR {}", challenge_rating(c)))
            .unwrap_or_default();
    let mut out = vec![
        style.title(&m.name),
        style.dim(format!("{size} {kind}{cr}").trim()),
        style.rule(),
        field(
            style,
            "Armor Class",
            &m.ac.map_or("—".into(), |v| v.to_string()),
        ),
        field(
            style,
            "Hit Points",
            &m.hp.map_or("—".into(), |v| v.to_string()),
        ),
        field(style, "Speed", &speed(m)),
        style.rule(),
    ];

    let a = &m.abilities;
    let scores = [a.str, a.dex, a.con, a.int, a.wis, a.cha];
    let header: String = ["STR", "DEX", "CON", "INT", "WIS", "CHA"]
        .iter()
        .map(|h| format!("{h:<9}"))
        .collect();
    out.push(style.bold(header.trim_end()));
    let values: String = scores
        .iter()
        .map(|s| match s {
            Some(v) => format!("{:<9}", format!("{v} ({})", modifier(*v))),
            None => format!("{:<9}", "—"),
        })
        .collect();
    out.push(values.trim_end().to_string());

    if !m.actions.is_empty() {
        out.push(style.rule());
        out.push(style.bold("Actions"));
        for (i, action) in m.actions.iter().enumerate() {
            let mut summary = Vec::new();
            if let Some(b) = action.attack_bonus {
                summary.push(format!("{} to hit", signed(i64::from(b))));
            }
            if let Some(dice) = &action.damage_dice {
                let bonus = action
                    .damage_bonus
                    .filter(|b| *b != 0)
                    .map(|b| signed(i64::from(b)))
                    .unwrap_or_default();
                summary.push(format!("{dice}{bonus} damage"));
            }
            let summary = if summary.is_empty() {
                String::new()
            } else {
                format!(" {}", style.cyan(&summary.join(", ")))
            };
            out.push(format!(
                "{} {}{summary}",
                style.dim(&format!("[{i}]")),
                style.bold(&action.name)
            ));
            if let Some(desc) = &action.desc {
                out.push(style.dim(&wrap(desc, style.width, 4)));
            }
        }
    }
    out.join("\n")
}

pub fn search(r: &SearchResults, style: &Style) -> String {
    let mut out = vec![style.title(&format!(
        "{} results for \"{}\"",
        capitalize(&r.kind),
        r.query
    ))];
    if r.results.is_empty() {
        out.push(style.dim("  no matches"));
        return out.join("\n");
    }
    let width = r
        .results
        .iter()
        .map(|h| h.name.chars().count())
        .max()
        .unwrap_or(0);
    for hit in &r.results {
        let pad = width - hit.name.chars().count();
        out.push(format!(
            "  {}{}  {}",
            style.bold(&hit.name),
            " ".repeat(pad),
            style.dim(&hit.slug)
        ));
    }
    if r.count > r.results.len() as u64 {
        out.push(style.dim(&format!(
            "  showing {} of {} matches",
            r.results.len(),
            r.count
        )));
    }
    out.join("\n")
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// One die's faces: kept in normal weight, dropped marked; natural 20/1 on
/// a d20 highlighted.
fn faces(roll: &DieRoll, style: &Style) -> String {
    let is_d20 = roll
        .die
        .trim_start_matches('-')
        .split(['k', 'd'])
        .nth(1)
        .is_some_and(|sides| sides == "20");
    let mut kept = roll.kept.iter().peekable();
    let parts: Vec<String> = roll
        .results
        .iter()
        .map(|v| {
            let text = v.to_string();
            if kept.peek() == Some(&v) {
                kept.next();
                match (is_d20, *v) {
                    (true, 20) => style.green(&text),
                    (true, 1) => style.red(&text),
                    _ => text,
                }
            } else {
                style.dropped(&text)
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

pub fn roll(r: &RollOutcome, style: &Style) -> String {
    let mut out = vec![format!(
        "{}  {}  {}",
        style.bold(&r.expr),
        style.dim("→"),
        style.title(&r.total.to_string())
    )];
    // Align dice and the "modifier" label in one column.
    let label_width = if r.modifier != 0 { "modifier".len() } else { 0 };
    let width = r
        .rolls
        .iter()
        .map(|d| d.die.len())
        .chain([label_width])
        .max()
        .unwrap_or(0);
    for d in &r.rolls {
        let sum: i64 = d.kept.iter().map(|v| i64::from(*v)).sum();
        let sum = if d.die.starts_with('-') { -sum } else { sum };
        out.push(format!(
            "  {:<width$}  {}  {}",
            d.die,
            faces(d, style),
            style.dim(&format!("= {sum}"))
        ));
    }
    if r.modifier != 0 {
        out.push(format!(
            "  {:<width$}  {}",
            "modifier",
            signed(r.modifier),
            width = width
        ));
    }
    out.join("\n")
}

pub fn attack(a: &AttackRoll, monster_name: &str, style: &Style) -> String {
    let natural = a.to_hit.natural.to_string();
    let natural = match a.to_hit.natural {
        20 => style.green(&natural),
        1 => style.red(&natural),
        _ => natural,
    };
    let mut out = vec![
        style.title(&format!("{monster_name}: {}", a.action)),
        format!(
            "{} d20 {natural} {} = {}",
            style.bold(&format!("{:<8}", "To hit")),
            signed(i64::from(a.to_hit.bonus)),
            style.bold(&a.to_hit.total.to_string())
        ),
    ];
    match &a.damage {
        Some(d) => {
            let dice: Vec<String> = d.rolls.iter().map(|r| faces(r, style)).collect();
            let modifier = if d.modifier != 0 {
                format!(" {}", signed(d.modifier))
            } else {
                String::new()
            };
            out.push(format!(
                "{} {} {}{modifier} = {}",
                style.bold(&format!("{:<8}", "Damage")),
                style.dim(&d.expr),
                dice.join(" "),
                style.bold(&d.total.to_string())
            ));
        }
        None => out.push(format!(
            "{} {}",
            style.bold(&format!("{:<8}", "Damage")),
            style.dim("none")
        )),
    }
    if a.critical {
        out.push(style.green("Natural 20: critical hit, damage dice doubled!"));
    } else if a.to_hit.natural == 1 {
        out.push(style.red("Natural 1: a miss, whatever the target's AC."));
    }
    out.join("\n")
}

/// An error line, with a caret under the bad character when the server
/// reported a position in `expr`.
pub fn error(message: &str, expr_and_position: Option<(&str, usize)>, style: &Style) -> String {
    match expr_and_position {
        Some((expr, pos)) => {
            // Strip the server's " at position N" suffix; the caret shows it.
            let message = message
                .rsplit_once(" at position ")
                .map_or(message, |(m, _)| m);
            format!(
                "{}\n  {expr}\n  {}{} {message}",
                style.red("error: invalid dice expression"),
                " ".repeat(pos),
                style.red("^")
            )
        }
        None => format!("{} {message}", style.red("error:")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open5e::models::{RawMonster, RawSpell};

    const S: Style = Style {
        color: false,
        width: 60,
    };

    fn fixture<T: serde::de::DeserializeOwned>(rel: &str) -> T {
        let p = format!("{}/tests/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn helpers() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(9), "9th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(challenge_rating(0.25), "1/4");
        assert_eq!(challenge_rating(0.125), "1/8");
        assert_eq!(challenge_rating(17.0), "17");
        assert_eq!(modifier(8), "-1");
        assert_eq!(modifier(10), "+0");
        assert_eq!(modifier(27), "+8");
        assert_eq!(modifier(1), "-5");
    }

    #[test]
    fn wrap_respects_width_and_indent() {
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let w = wrap(text, 24, 2);
        assert!(
            w.lines()
                .all(|l| l.chars().count() <= 24 && l.starts_with("  ")),
            "{w}"
        );
        assert_eq!(w.split_whitespace().count(), 12);
        assert_eq!(wrap("a\n\nb", 40, 0), "a\n\nb");
    }

    #[test]
    fn renders_fireball() {
        let s = Spell::from(fixture::<RawSpell>("spells/fireball.json"));
        let out = spell(&s, &S);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Fireball");
        assert_eq!(lines[1], "3rd-level evocation");
        assert!(out.contains("Casting Time  1 action"), "{out}");
        assert!(out.contains("Range         150 feet"), "{out}");
        assert!(out.contains("8d6 fire damage"));
        assert!(out.lines().all(|l| l.chars().count() <= 60), "{out}");
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn renders_spell_tags() {
        let mut s = Spell::from(fixture::<RawSpell>("spells/shield.json"));
        s.concentration = Some(true);
        s.ritual = Some(true);
        s.level = Some(0);
        let out = spell(&s, &S);
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("Abjuration cantrip  [concentration, ritual]"),
            "{out}"
        );
    }

    #[test]
    fn renders_goblin_stat_block() {
        let m = Monster::from(fixture::<RawMonster>("monsters/goblin.json"));
        let out = monster(&m, &S);
        assert!(out.contains("Small humanoid, CR 1/4"), "{out}");
        assert!(out.contains("Armor Class   15"));
        assert!(out.contains("Hit Points    7"));
        assert!(out.contains("Speed         30 ft."));
        assert!(
            out.contains("STR      DEX      CON      INT      WIS      CHA"),
            "{out}"
        );
        assert!(out.contains("8 (-1)   14 (+2)  10 (+0)"), "{out}");
        assert!(
            out.contains("[0] Scimitar +4 to hit, 1d6+2 damage"),
            "{out}"
        );
    }

    #[test]
    fn renders_dragon_speeds_and_actions_without_attacks() {
        let m = Monster::from(fixture::<RawMonster>("monsters/adult-red-dragon.json"));
        let out = monster(&m, &S);
        assert!(out.contains("40 ft., climb 40 ft., fly 80 ft."), "{out}");
        assert!(out.contains("[0] Multiattack\n"), "{out}");
        assert!(
            out.contains("[1] Bite +14 to hit, 2d10+2d6+8 damage"),
            "{out}"
        );
    }

    #[test]
    fn renders_roll_with_dropped_dice() {
        let r = crate::dice::roll("4d6kh3+2", Some(42)).unwrap();
        let out = roll(&r, &S);
        assert_eq!(
            out,
            "4d6kh3+2  →  12\n  4d6kh3    [(1), 4, 2, 4]  = 10\n  modifier  +2"
        );
    }

    #[test]
    fn renders_subtracted_dice_as_negative() {
        let r = crate::dice::roll("1d4-1d4", Some(1)).unwrap();
        let out = roll(&r, &S);
        let second = out.lines().nth(2).unwrap();
        assert!(second.trim_start().starts_with("-1d4"), "{out}");
        let v = r.rolls[1].kept[0];
        assert!(second.ends_with(&format!("= -{v}")), "{out}");
    }

    #[test]
    fn renders_attack_and_critical() {
        let m = Monster::from(fixture::<RawMonster>("monsters/goblin.json"));
        let a = crate::api::roll_attack(&m, 0, Some(7)).unwrap();
        let out = attack(&a, &m.name, &S);
        assert_eq!(
            out,
            "Goblin: Scimitar\nTo hit   d20 9 +4 = 13\nDamage   1d6+2 [1] +2 = 3"
        );
        let crit = crate::api::roll_attack(&m, 0, Some(27)).unwrap();
        let out = attack(&crit, &m.name, &S);
        assert!(out.contains("Damage   2d6+2 [2, 2] +2 = 6"), "{out}");
        assert!(out.ends_with("critical hit, damage dice doubled!"), "{out}");
    }

    #[test]
    fn renders_search_with_truncation_note() {
        let r: crate::open5e::models::RawSearchPage = fixture("search/spells-fire.json");
        let out = search(&r.into_results("spell", "fire"), &S);
        assert!(out.starts_with("Spell results for \"fire\""));
        assert!(out.contains("Alchemical Form"));
        assert!(out.contains("alchemical-form"));
        assert!(out.contains("showing 5 of"), "{out}");
    }

    #[test]
    fn renders_error_caret_under_position() {
        let out = error(
            "dice count must be between 1 and 100 at position 8",
            Some(("2d20kh1+101d6", 8)),
            &S,
        );
        assert_eq!(
            out,
            "error: invalid dice expression\n  2d20kh1+101d6\n          ^ dice count must be between 1 and 100"
        );
        assert_eq!(error("no spell", None, &S), "error: no spell");
    }

    #[test]
    fn color_adds_ansi_codes() {
        let c = Style {
            color: true,
            width: 60,
        };
        let r = crate::dice::roll("2d20kh1", Some(42)).unwrap();
        assert!(roll(&r, &c).contains("\x1b["));
    }
}
