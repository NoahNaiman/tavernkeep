//! Dice engine: a hand-written parser, an evaluator and seeded RNG helpers.

mod eval;
pub mod parser;

use rand::SeedableRng;
use rand::rngs::StdRng;
use serde::Serialize;

pub use eval::evaluate;
pub use parser::parse;

/// A parsed dice expression: a signed sum of terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub terms: Vec<Term>,
}

/// One signed term of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// `1` for added terms, `-1` for subtracted ones.
    pub sign: i64,
    pub kind: TermKind,
    /// 0-based position of the term's first character in the input.
    pub position: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermKind {
    Const(i64),
    Dice(DiceSpec),
}

/// `count` dice with `sides` faces, optionally keeping or dropping some.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiceSpec {
    pub count: u32,
    pub sides: u32,
    pub select: Option<Select>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    KeepHighest(u32),
    KeepLowest(u32),
    DropHighest(u32),
    DropLowest(u32),
}

impl DiceSpec {
    pub fn advantage() -> Self {
        Self {
            count: 2,
            sides: 20,
            select: Some(Select::KeepHighest(1)),
        }
    }

    pub fn disadvantage() -> Self {
        Self {
            count: 2,
            sides: 20,
            select: Some(Select::KeepLowest(1)),
        }
    }

    /// How many dice survive the keep/drop selector.
    pub fn kept_count(&self) -> u32 {
        match self.select {
            None => self.count,
            Some(Select::KeepHighest(k) | Select::KeepLowest(k)) => k,
            Some(Select::DropHighest(k) | Select::DropLowest(k)) => self.count - k,
        }
    }

    /// Canonical notation, e.g. `2d20kh1`.
    pub fn notation(&self) -> String {
        let base = format!("{}d{}", self.count, self.sides);
        match self.select {
            None => base,
            Some(Select::KeepHighest(k)) => format!("{base}kh{k}"),
            Some(Select::KeepLowest(k)) => format!("{base}kl{k}"),
            Some(Select::DropHighest(k)) => format!("{base}dh{k}"),
            Some(Select::DropLowest(k)) => format!("{base}dl{k}"),
        }
    }
}

impl Expr {
    /// Smallest possible total.
    pub fn min_total(&self) -> i64 {
        self.terms.iter().map(|t| t.bounds().0).sum()
    }

    /// Largest possible total.
    pub fn max_total(&self) -> i64 {
        self.terms.iter().map(|t| t.bounds().1).sum()
    }

    /// Canonical notation for the whole expression, e.g. `2d6+1d8-1`.
    pub fn notation(&self) -> String {
        let mut out = String::new();
        for (i, t) in self.terms.iter().enumerate() {
            if t.sign < 0 {
                out.push('-');
            } else if i > 0 {
                out.push('+');
            }
            match &t.kind {
                TermKind::Const(c) => out.push_str(&c.to_string()),
                TermKind::Dice(d) => out.push_str(&d.notation()),
            }
        }
        out
    }

    /// A copy with `value` added as a constant term (no-op for 0).
    pub fn plus_const(&self, value: i64) -> Expr {
        let mut e = self.clone();
        if value != 0 {
            e.terms.push(Term {
                sign: value.signum(),
                kind: TermKind::Const(value.abs()),
                position: 0,
            });
        }
        e
    }

    /// A copy with every dice count doubled (critical hits). Selectors are
    /// scaled with the count so `kh`/`dl` keep meaning the same thing.
    pub fn with_doubled_dice(&self) -> Expr {
        let terms = self
            .terms
            .iter()
            .map(|t| match &t.kind {
                TermKind::Dice(d) => Term {
                    kind: TermKind::Dice(DiceSpec {
                        count: d.count * 2,
                        sides: d.sides,
                        select: d.select.map(|s| match s {
                            Select::KeepHighest(k) => Select::KeepHighest(k * 2),
                            Select::KeepLowest(k) => Select::KeepLowest(k * 2),
                            Select::DropHighest(k) => Select::DropHighest(k * 2),
                            Select::DropLowest(k) => Select::DropLowest(k * 2),
                        }),
                    }),
                    ..t.clone()
                },
                TermKind::Const(_) => t.clone(),
            })
            .collect();
        Expr { terms }
    }
}

impl Term {
    fn bounds(&self) -> (i64, i64) {
        let (lo, hi) = match &self.kind {
            TermKind::Const(c) => (*c, *c),
            TermKind::Dice(d) => {
                let k = i64::from(d.kept_count());
                (k, k * i64::from(d.sides))
            }
        };
        if self.sign < 0 { (-hi, -lo) } else { (lo, hi) }
    }
}

/// A parse or limit error, pointing at a 0-based character position.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at position {position}")]
pub struct DiceError {
    pub message: String,
    pub position: usize,
}

impl DiceError {
    pub fn new(message: impl Into<String>, position: usize) -> Self {
        Self {
            message: message.into(),
            position,
        }
    }
}

/// The result of rolling one dice term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DieRoll {
    /// Canonical notation of the term, prefixed with `-` when subtracted.
    pub die: String,
    /// Every die rolled, in roll order.
    pub results: Vec<u32>,
    /// The dice that count toward the total, in roll order.
    pub kept: Vec<u32>,
}

/// The full result of evaluating an expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollOutcome {
    pub expr: String,
    pub total: i64,
    pub rolls: Vec<DieRoll>,
    /// Sum of the signed integer terms.
    pub modifier: i64,
}

/// A deterministic RNG for `seed`, or a randomly seeded one.
pub fn rng_for(seed: Option<u64>) -> StdRng {
    StdRng::seed_from_u64(seed.unwrap_or_else(rand::random))
}

/// Parse and roll `input` in one step.
pub fn roll(input: &str, seed: Option<u64>) -> Result<RollOutcome, DiceError> {
    let expr = parse(input)?;
    let mut rng = rng_for(seed);
    Ok(evaluate(input.trim(), &expr, &mut rng))
}
