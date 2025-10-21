//! Property tests for the dice engine.

use proptest::prelude::*;
use tavernkeep::dice::{self, DiceSpec, Select, TermKind, evaluate, parse, rng_for};

/// A valid dice term rendered as text, plus the number of dice it drops.
fn dice_term() -> impl Strategy<Value = (String, u32, u32)> {
    (1u32..=100, 2u32..=1000, 0u8..5, any::<u32>()).prop_map(|(count, sides, kind, k)| {
        let base = format!("{count}d{sides}");
        match kind {
            0 => (base, count, 0),
            1 | 2 => {
                let keep = k % count + 1;
                let op = if kind == 1 { "kh" } else { "kl" };
                (format!("{base}{op}{keep}"), count, count - keep)
            }
            _ if count == 1 => (base, count, 0),
            _ => {
                let drop = k % (count - 1) + 1;
                let op = if kind == 3 { "dh" } else { "dl" };
                (format!("{base}{op}{drop}"), count, drop)
            }
        }
    })
}

fn term() -> impl Strategy<Value = (String, Option<(u32, u32)>)> {
    prop_oneof![
        dice_term().prop_map(|(s, n, d)| (s, Some((n, d)))),
        (0u32..=1000).prop_map(|c| (c.to_string(), None)),
        Just(("adv".to_string(), Some((2, 1)))),
        Just(("dis".to_string(), Some((2, 1)))),
    ]
}

/// A valid expression and, for each dice term, (count, dropped).
fn expression() -> impl Strategy<Value = (String, Vec<(u32, u32)>)> {
    prop::collection::vec((term(), any::<bool>()), 1..=10).prop_map(|terms| {
        let mut out = String::new();
        let mut dice = Vec::new();
        for (i, ((text, info), minus)) in terms.into_iter().enumerate() {
            if i > 0 || minus {
                out.push_str(if minus { " - " } else { " + " });
            }
            out.push_str(&text);
            dice.extend(info);
        }
        (out, dice)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn totals_fall_within_bounds((input, _) in expression(), seed in any::<u64>()) {
        let expr = parse(&input).unwrap();
        let out = evaluate(&input, &expr, &mut rng_for(Some(seed)));
        prop_assert!(out.total >= expr.min_total(), "{} < {}", out.total, expr.min_total());
        prop_assert!(out.total <= expr.max_total(), "{} > {}", out.total, expr.max_total());
    }

    #[test]
    fn kept_count_is_n_minus_dropped((input, dice_info) in expression(), seed in any::<u64>()) {
        let out = dice::roll(&input, Some(seed)).unwrap();
        prop_assert_eq!(out.rolls.len(), dice_info.len());
        for (roll, (count, dropped)) in out.rolls.iter().zip(dice_info) {
            prop_assert_eq!(roll.results.len() as u32, count);
            prop_assert_eq!(roll.kept.len() as u32, count - dropped);
        }
    }

    #[test]
    fn results_respect_die_size((input, _) in expression(), seed in any::<u64>()) {
        let expr = parse(&input).unwrap();
        let out = evaluate(&input, &expr, &mut rng_for(Some(seed)));
        let specs: Vec<DiceSpec> = expr.terms.iter().filter_map(|t| match t.kind {
            TermKind::Dice(d) => Some(d),
            TermKind::Const(_) => None,
        }).collect();
        for (roll, spec) in out.rolls.iter().zip(specs) {
            prop_assert!(roll.results.iter().all(|v| (1..=spec.sides).contains(v)));
            if let Some(Select::KeepHighest(_)) = spec.select {
                let min_kept = roll.kept.iter().min().copied().unwrap_or(0);
                let dropped_max = {
                    let mut rest = roll.results.clone();
                    for k in &roll.kept {
                        let i = rest.iter().position(|v| v == k).unwrap();
                        rest.remove(i);
                    }
                    rest.into_iter().max().unwrap_or(0)
                };
                prop_assert!(dropped_max <= min_kept);
            }
        }
    }

    #[test]
    fn same_seed_same_output((input, _) in expression(), seed in any::<u64>()) {
        prop_assert_eq!(dice::roll(&input, Some(seed)), dice::roll(&input, Some(seed)));
    }

    #[test]
    fn random_strings_never_panic(input in "\\PC{0,300}") {
        if let Err(e) = parse(&input) {
            prop_assert!(e.position <= input.chars().count());
        }
    }

    #[test]
    fn dice_alphabet_strings_never_panic(input in "[0-9dkhlasvi+\\- ]{0,40}") {
        if let Ok(expr) = parse(&input) {
            let out = evaluate(&input, &expr, &mut rng_for(Some(1)));
            prop_assert!(out.total >= expr.min_total() && out.total <= expr.max_total());
        }
    }

    #[test]
    fn error_positions_are_within_input(input in "[0-9dkhl+\\- x]{0,40}") {
        if let Err(e) = parse(&input) {
            prop_assert!(e.position <= input.len(), "{} > {}", e.position, input.len());
        }
    }
}
