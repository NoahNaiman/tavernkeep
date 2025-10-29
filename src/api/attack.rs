//! `GET /monsters/{slug}/attack/{action_index}`: roll a monster's attack.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState};
use crate::dice::{self, Expr, RollOutcome, rng_for};
use crate::open5e::{Action, Monster};

#[derive(Debug, Deserialize)]
pub struct AttackParams {
    seed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToHit {
    /// The d20 face rolled.
    pub natural: u32,
    pub bonus: i32,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttackRoll {
    pub monster: String,
    pub action: String,
    pub to_hit: ToHit,
    /// Natural 20: damage dice are doubled.
    pub critical: bool,
    /// `None` when the action has no damage dice.
    pub damage: Option<RollOutcome>,
}

pub async fn attack(
    State(state): State<AppState>,
    Path((slug, index)): Path<(String, String)>,
    Query(params): Query<AttackParams>,
) -> Result<Json<AttackRoll>, ApiError> {
    let index: usize = index
        .parse()
        .map_err(|_| ApiError::BadRequest("action_index must be a non-negative integer".into()))?;
    let lookup = state.service.monster(&slug).await?;
    let roll = roll_attack(&lookup.doc.value, index, params.seed)?;
    metrics::counter!("tk_rolls_total").increment(1);
    Ok(Json(roll))
}

/// Roll to-hit (d20 + attack bonus) and damage for `monster.actions[index]`.
/// The d20 is rolled first, then damage, from one RNG seeded by `seed`.
pub fn roll_attack(
    monster: &Monster,
    index: usize,
    seed: Option<u64>,
) -> Result<AttackRoll, ApiError> {
    let action: &Action = monster.actions.get(index).ok_or_else(|| {
        ApiError::NotFound(format!(
            "{} has {} actions (indices 0..{})",
            monster.name,
            monster.actions.len(),
            monster.actions.len()
        ))
    })?;
    let bonus = action.attack_bonus.ok_or_else(|| ApiError::Unprocessable {
        message: format!("{:?} has no attack roll", action.name),
        position: None,
    })?;
    let damage_expr = action
        .damage_dice
        .as_deref()
        .map(|d| {
            dice::parse(d).map_err(|e| ApiError::Unprocessable {
                message: format!("cannot roll damage dice {d:?}: {e}"),
                position: None,
            })
        })
        .transpose()?;

    let mut rng = rng_for(seed);
    let d20 = dice::evaluate("1d20", &dice::parse("1d20").expect("valid"), &mut rng);
    let natural = d20.rolls[0].kept[0];
    let critical = natural == 20;

    let damage = damage_expr.map(|expr: Expr| {
        let expr = if critical {
            expr.with_doubled_dice()
        } else {
            expr
        };
        let expr = expr.plus_const(i64::from(action.damage_bonus.unwrap_or(0)));
        dice::evaluate(&expr.notation(), &expr, &mut rng)
    });

    Ok(AttackRoll {
        monster: monster.slug.clone(),
        action: action.name.clone(),
        to_hit: ToHit {
            natural,
            bonus,
            total: i64::from(natural) + i64::from(bonus),
        },
        critical,
        damage,
    })
}
