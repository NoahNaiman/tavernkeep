//! Evaluates a parsed [`Expr`] with a caller-supplied RNG.

use rand::{Rng, RngExt};

use super::{DiceSpec, DieRoll, Expr, RollOutcome, Select, TermKind};

/// Roll every dice term of `expr` and sum the result.
///
/// `label` is echoed back as the outcome's `expr`. The expression is assumed
/// to be valid (it came from [`super::parse`] or a transformation of it).
pub fn evaluate<R: Rng + ?Sized>(label: &str, expr: &Expr, rng: &mut R) -> RollOutcome {
    let mut total: i64 = 0;
    let mut modifier: i64 = 0;
    let mut rolls = Vec::new();
    for term in &expr.terms {
        match &term.kind {
            TermKind::Const(c) => {
                modifier += term.sign * c;
                total += term.sign * c;
            }
            TermKind::Dice(spec) => {
                let roll = roll_dice(spec, rng);
                let kept_sum: i64 = roll.kept.iter().map(|v| i64::from(*v)).sum();
                total += term.sign * kept_sum;
                let die = if term.sign < 0 {
                    format!("-{}", spec.notation())
                } else {
                    spec.notation()
                };
                rolls.push(DieRoll { die, ..roll });
            }
        }
    }
    RollOutcome {
        expr: label.to_string(),
        total,
        rolls,
        modifier,
    }
}

fn roll_dice<R: Rng + ?Sized>(spec: &DiceSpec, rng: &mut R) -> DieRoll {
    let results: Vec<u32> = (0..spec.count)
        .map(|_| rng.random_range(1..=spec.sides))
        .collect();

    // Order indices from lowest to highest value; ties keep roll order so the
    // choice of which equal die is dropped is deterministic.
    let mut order: Vec<usize> = (0..results.len()).collect();
    order.sort_by_key(|&i| results[i]);

    let n = results.len();
    let mut keep = vec![true; n];
    let mut drop_indices = |indices: &[usize]| {
        for &i in indices {
            keep[i] = false;
        }
    };
    match spec.select {
        None => {}
        Some(Select::KeepHighest(k)) => drop_indices(&order[..n - k as usize]),
        Some(Select::KeepLowest(k)) => drop_indices(&order[k as usize..]),
        Some(Select::DropHighest(k)) => drop_indices(&order[n - k as usize..]),
        Some(Select::DropLowest(k)) => drop_indices(&order[..k as usize]),
    }

    let kept = results
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(v, _)| *v)
        .collect();
    DieRoll {
        die: spec.notation(),
        results,
        kept,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{parse, rng_for, roll};
    use super::*;

    #[test]
    fn keep_highest_keeps_the_max() {
        let expr = parse("4d6kh3").unwrap();
        for seed in 0..200 {
            let out = evaluate("4d6kh3", &expr, &mut rng_for(Some(seed)));
            let roll = &out.rolls[0];
            let mut sorted = roll.results.clone();
            sorted.sort_unstable();
            let mut kept = roll.kept.clone();
            kept.sort_unstable();
            assert_eq!(kept, sorted[1..].to_vec());
        }
    }

    #[test]
    fn keep_lowest_and_drop_variants() {
        for (input, pick) in [("4d6kl1", 0..1), ("4d6dh1", 0..3), ("4d6dl1", 1..4)] {
            let expr = parse(input).unwrap();
            for seed in 0..100 {
                let out = evaluate(input, &expr, &mut rng_for(Some(seed)));
                let mut sorted = out.rolls[0].results.clone();
                sorted.sort_unstable();
                let mut kept = out.rolls[0].kept.clone();
                kept.sort_unstable();
                assert_eq!(kept, sorted[pick.clone()].to_vec(), "{input} seed {seed}");
            }
        }
    }

    #[test]
    fn modifier_and_subtraction_are_applied() {
        let out = roll("1d4 - 1d4 + 5 - 2", Some(7)).unwrap();
        assert_eq!(out.modifier, 3);
        let a = i64::from(out.rolls[0].kept[0]);
        let b = i64::from(out.rolls[1].kept[0]);
        assert_eq!(out.total, a - b + 3);
        assert_eq!(out.rolls[1].die, "-1d4");
    }

    #[test]
    fn advantage_reports_canonical_die() {
        let out = roll("adv+3", Some(1)).unwrap();
        assert_eq!(out.rolls[0].die, "2d20kh1");
        assert_eq!(out.rolls[0].results.len(), 2);
        assert_eq!(out.rolls[0].kept.len(), 1);
        assert_eq!(out.total, i64::from(out.rolls[0].kept[0]) + 3);
        assert_eq!(out.expr, "adv+3");
    }

    #[test]
    fn same_seed_same_outcome() {
        assert_eq!(roll("8d6+2d20kh1", Some(42)), roll("8d6+2d20kh1", Some(42)));
    }

    #[test]
    fn doubled_dice_doubles_counts_and_keeps() {
        let doubled = parse("2d6+1d8kh1+3").unwrap().with_doubled_dice();
        let out = evaluate("crit", &doubled, &mut rng_for(Some(3)));
        assert_eq!(out.rolls[0].die, "4d6");
        assert_eq!(out.rolls[1].die, "2d8kh2");
        assert_eq!(out.modifier, 3);
    }
}
