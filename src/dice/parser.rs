//! Hand-written recursive-descent parser for dice notation.
//!
//! Grammar (whitespace is allowed between tokens):
//!
//! ```text
//! expr   := sign? term (op term)*
//! op     := '+' | '-'
//! sign   := '+' | '-'
//! term   := 'adv' | 'dis' | dice | int
//! dice   := int? 'd' int select?
//! select := ('kh' | 'kl' | 'dh' | 'dl') int?
//! int    := [0-9]+
//! ```
//!
//! `adv` is shorthand for `2d20kh1` and `dis` for `2d20kl1`. A missing dice
//! count means 1 (`d20` == `1d20`) and a missing keep/drop count means 1
//! (`2d20kh` == `2d20kh1`). Input is case-insensitive.

use super::{DiceError, DiceSpec, Expr, Select, Term, TermKind};

/// Maximum number of dice in a single term.
pub const MAX_COUNT: u32 = 100;
/// Smallest allowed die size.
pub const MIN_SIDES: u32 = 2;
/// Largest allowed die size.
pub const MAX_SIDES: u32 = 1000;
/// Maximum number of terms (dice terms and integer terms) in one expression.
pub const MAX_TERMS: usize = 10;
/// Largest integer literal accepted anywhere in an expression.
pub const MAX_INT: u32 = 1_000_000;
/// Longest accepted input, to bound parser work on hostile input.
pub const MAX_INPUT_LEN: usize = 256;

/// Parse a dice expression into an [`Expr`], enforcing all limits.
///
/// Errors carry the 0-based position of the offending character.
pub fn parse(input: &str) -> Result<Expr, DiceError> {
    // The grammar is pure ASCII, so the first non-ASCII character is always
    // an error. Rejecting it first keeps every later position a valid
    // character index (byte offsets equal char offsets in ASCII).
    if let Some((position, c)) = input.chars().enumerate().find(|(_, c)| !c.is_ascii()) {
        return Err(DiceError::new(
            format!("unexpected character '{c}'"),
            position,
        ));
    }
    if input.len() > MAX_INPUT_LEN {
        return Err(DiceError::new(
            format!("expression is longer than {MAX_INPUT_LEN} characters"),
            MAX_INPUT_LEN,
        ));
    }
    let lowered = input.to_ascii_lowercase();
    let mut p = Parser {
        src: lowered.as_bytes(),
        pos: 0,
    };
    p.expr()
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn starts_with(&self, word: &str) -> bool {
        self.src[self.pos..].starts_with(word.as_bytes())
    }

    fn unexpected(&self, expected: &str) -> DiceError {
        match self.peek() {
            None => DiceError::new(
                format!("unexpected end of input, expected {expected}"),
                self.pos,
            ),
            Some(c) if c.is_ascii_graphic() => DiceError::new(
                format!("unexpected character '{}', expected {expected}", c as char),
                self.pos,
            ),
            Some(_) => DiceError::new(
                format!("unexpected character, expected {expected}"),
                self.pos,
            ),
        }
    }

    fn expr(&mut self) -> Result<Expr, DiceError> {
        let mut terms = Vec::new();
        self.skip_ws();
        let mut sign: i64 = 1;
        match self.peek() {
            Some(b'-') => {
                sign = -1;
                self.pos += 1;
            }
            Some(b'+') => self.pos += 1,
            _ => {}
        }
        loop {
            self.skip_ws();
            let start = self.pos;
            if terms.len() == MAX_TERMS {
                return Err(DiceError::new(
                    format!("too many terms (at most {MAX_TERMS})"),
                    start,
                ));
            }
            let kind = self.term()?;
            terms.push(Term {
                sign,
                kind,
                position: start,
            });
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b'+') => sign = 1,
                Some(b'-') => sign = -1,
                Some(_) => return Err(self.unexpected("'+', '-' or end of expression")),
            }
            self.pos += 1;
        }
        Ok(Expr { terms })
    }

    fn term(&mut self) -> Result<TermKind, DiceError> {
        if self.starts_with("adv") {
            self.pos += 3;
            return Ok(TermKind::Dice(DiceSpec::advantage()));
        }
        if self.starts_with("dis") {
            self.pos += 3;
            return Ok(TermKind::Dice(DiceSpec::disadvantage()));
        }
        let count_pos = self.pos;
        let count = match self.peek() {
            Some(c) if c.is_ascii_digit() => Some(self.int()?),
            Some(b'd') => None,
            _ => return Err(self.unexpected("a number, a dice term, 'adv' or 'dis'")),
        };
        if self.peek() != Some(b'd') {
            // A plain integer term; `count` is always Some here.
            return Ok(TermKind::Const(i64::from(count.unwrap_or(0))));
        }
        let count = count.unwrap_or(1);
        if count == 0 || count > MAX_COUNT {
            return Err(DiceError::new(
                format!("dice count must be between 1 and {MAX_COUNT}"),
                count_pos,
            ));
        }
        self.pos += 1; // 'd'
        let sides_pos = self.pos;
        if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            return Err(self.unexpected("number of sides"));
        }
        let sides = self.int()?;
        if !(MIN_SIDES..=MAX_SIDES).contains(&sides) {
            return Err(DiceError::new(
                format!("die sides must be between {MIN_SIDES} and {MAX_SIDES}"),
                sides_pos,
            ));
        }
        let select = self.select(count)?;
        Ok(TermKind::Dice(DiceSpec {
            count,
            sides,
            select,
        }))
    }

    fn select(&mut self, count: u32) -> Result<Option<Select>, DiceError> {
        let make: fn(u32) -> Select = if self.starts_with("kh") {
            Select::KeepHighest
        } else if self.starts_with("kl") {
            Select::KeepLowest
        } else if self.starts_with("dh") {
            Select::DropHighest
        } else if self.starts_with("dl") {
            Select::DropLowest
        } else {
            return Ok(None);
        };
        let op_pos = self.pos;
        self.pos += 2;
        let n_pos = self.pos;
        let n = match self.peek() {
            Some(c) if c.is_ascii_digit() => self.int()?,
            _ => 1,
        };
        let select = make(n);
        let err_pos = if n_pos == self.pos { op_pos } else { n_pos };
        match select {
            Select::KeepHighest(k) | Select::KeepLowest(k) if k == 0 || k > count => Err(
                DiceError::new(format!("must keep between 1 and {count} dice"), err_pos),
            ),
            Select::DropHighest(k) | Select::DropLowest(k) if k == 0 || k >= count => {
                Err(DiceError::new(
                    if count == 1 {
                        "cannot drop dice from a single die".to_string()
                    } else {
                        format!("must drop between 1 and {} dice", count - 1)
                    },
                    err_pos,
                ))
            }
            s => Ok(Some(s)),
        }
    }

    fn int(&mut self) -> Result<u32, DiceError> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u32::from(c - b'0')))
                .filter(|v| *v <= MAX_INT)
                .ok_or_else(|| DiceError::new(format!("number larger than {MAX_INT}"), start))?;
            self.pos += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dice(count: u32, sides: u32, select: Option<Select>) -> TermKind {
        TermKind::Dice(DiceSpec {
            count,
            sides,
            select,
        })
    }

    fn kinds(input: &str) -> Vec<(i64, TermKind)> {
        parse(input)
            .unwrap()
            .terms
            .into_iter()
            .map(|t| (t.sign, t.kind))
            .collect()
    }

    fn err(input: &str) -> DiceError {
        parse(input).unwrap_err()
    }

    #[test]
    fn plain_dice() {
        assert_eq!(kinds("2d6"), vec![(1, dice(2, 6, None))]);
    }

    #[test]
    fn implicit_count_is_one() {
        assert_eq!(kinds("d20"), vec![(1, dice(1, 20, None))]);
    }

    #[test]
    fn keep_and_drop_selectors() {
        assert_eq!(
            kinds("4d6kh3"),
            vec![(1, dice(4, 6, Some(Select::KeepHighest(3))))]
        );
        assert_eq!(
            kinds("2d20kl1"),
            vec![(1, dice(2, 20, Some(Select::KeepLowest(1))))]
        );
        assert_eq!(
            kinds("4d6dl1"),
            vec![(1, dice(4, 6, Some(Select::DropLowest(1))))]
        );
        assert_eq!(
            kinds("3d8dh2"),
            vec![(1, dice(3, 8, Some(Select::DropHighest(2))))]
        );
    }

    #[test]
    fn selector_count_defaults_to_one() {
        assert_eq!(
            kinds("2d20kh"),
            vec![(1, dice(2, 20, Some(Select::KeepHighest(1))))]
        );
    }

    #[test]
    fn modifiers_and_dice_sums() {
        assert_eq!(
            kinds("1d8 + 2d6 - 3"),
            vec![
                (1, dice(1, 8, None)),
                (1, dice(2, 6, None)),
                (-1, TermKind::Const(3))
            ]
        );
        assert_eq!(
            kinds("1d20-1d4"),
            vec![(1, dice(1, 20, None)), (-1, dice(1, 4, None))]
        );
    }

    #[test]
    fn leading_sign() {
        assert_eq!(
            kinds("-2+1d4"),
            vec![(-1, TermKind::Const(2)), (1, dice(1, 4, None))]
        );
        assert_eq!(kinds("+5"), vec![(1, TermKind::Const(5))]);
    }

    #[test]
    fn advantage_and_disadvantage_shortcuts() {
        assert_eq!(
            kinds("adv"),
            vec![(1, dice(2, 20, Some(Select::KeepHighest(1))))]
        );
        assert_eq!(
            kinds("dis"),
            vec![(1, dice(2, 20, Some(Select::KeepLowest(1))))]
        );
        assert_eq!(
            kinds("adv+3"),
            vec![
                (1, dice(2, 20, Some(Select::KeepHighest(1)))),
                (1, TermKind::Const(3))
            ]
        );
    }

    #[test]
    fn case_and_whitespace_insensitive() {
        assert_eq!(kinds("  2D20KH1 +  5 "), kinds("2d20kh1+5"));
        assert_eq!(kinds("ADV"), kinds("adv"));
    }

    #[test]
    fn term_positions_are_recorded() {
        let e = parse("1d4 + 2d6").unwrap();
        assert_eq!(e.terms[0].position, 0);
        assert_eq!(e.terms[1].position, 6);
    }

    #[test]
    fn limits_at_boundaries_are_accepted() {
        assert!(parse("100d1000").is_ok());
        assert!(parse("1d2").is_ok());
        assert!(parse(&["1"; MAX_TERMS].join("+")).is_ok());
    }

    #[test]
    fn too_many_dice_points_at_count() {
        let e = err("1d4+101d6");
        assert_eq!(e.position, 4);
        assert!(e.message.contains("dice count"), "{}", e.message);
    }

    #[test]
    fn zero_dice_points_at_count() {
        assert_eq!(err("0d6").position, 0);
    }

    #[test]
    fn sides_out_of_range_points_at_sides() {
        let e = err("2d1001");
        assert_eq!(e.position, 2);
        assert!(e.message.contains("sides"), "{}", e.message);
        assert_eq!(err("3d1").position, 2);
    }

    #[test]
    fn too_many_terms_points_at_eleventh_term() {
        let input = ["1"; MAX_TERMS + 1].join("+");
        let e = err(&input);
        assert_eq!(e.position, 2 * MAX_TERMS);
        assert!(e.message.contains("too many terms"), "{}", e.message);
    }

    #[test]
    fn invalid_keep_counts_point_at_number() {
        assert_eq!(err("2d20kh3").position, 6);
        assert_eq!(err("2d20kh0").position, 6);
        assert_eq!(err("2d20dl2").position, 6);
    }

    #[test]
    fn drop_from_single_die_points_at_operator() {
        let e = err("1d20dl");
        assert_eq!(e.position, 4);
        assert!(e.message.contains("single die"), "{}", e.message);
    }

    #[test]
    fn missing_sides_points_after_d() {
        let e = err("2d");
        assert_eq!(e.position, 2);
        assert!(e.message.contains("end of input"), "{}", e.message);
    }

    #[test]
    fn unexpected_characters() {
        assert_eq!(err("2d6x").position, 3);
        assert_eq!(err("2d6 +").position, 5);
        assert_eq!(err("").position, 0);
        assert_eq!(err("+").position, 1);
        assert_eq!(err("2d6++1").position, 4);
        assert_eq!(err("2d6 3").position, 4);
        assert_eq!(err("é").position, 0);
    }

    #[test]
    fn huge_numbers_are_rejected_at_their_start() {
        let e = err("1+99999999999");
        assert_eq!(e.position, 2);
        assert!(e.message.contains("larger"), "{}", e.message);
    }

    #[test]
    fn overlong_input_is_rejected() {
        let input = "1+".repeat(200);
        let e = err(&input);
        assert!(e.message.contains("longer"));
        assert_eq!(e.position, MAX_INPUT_LEN);
    }

    #[test]
    fn non_ascii_is_reported_at_its_character_index() {
        let e = err("1d6+é");
        assert_eq!(e.position, 4);
        let e = err(&"é".repeat(129));
        assert_eq!(e.position, 0);
    }
}
