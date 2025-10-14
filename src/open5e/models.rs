//! Open5e response shapes and the small internal models they map to.
//!
//! Unknown upstream fields are ignored; missing optional fields become `None`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// A spell as served by Tavernkeep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spell {
    pub slug: String,
    pub name: String,
    pub level: Option<u8>,
    pub school: Option<String>,
    pub casting_time: Option<String>,
    pub range: Option<String>,
    pub duration: Option<String>,
    pub components: Option<String>,
    pub concentration: Option<bool>,
    pub ritual: Option<bool>,
    pub description: Option<String>,
}

/// A monster as served by Tavernkeep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monster {
    pub slug: String,
    pub name: String,
    pub size: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub cr: Option<f64>,
    pub ac: Option<u32>,
    pub hp: Option<u32>,
    /// Movement modes in feet, e.g. `{"walk": 30, "fly": 60}`.
    pub speed: BTreeMap<String, u32>,
    pub abilities: Abilities,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abilities {
    pub str: Option<u8>,
    pub dex: Option<u8>,
    pub con: Option<u8>,
    pub int: Option<u8>,
    pub wis: Option<u8>,
    pub cha: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    pub desc: Option<String>,
    pub attack_bonus: Option<i32>,
    pub damage_dice: Option<String>,
    /// Flat damage added to `damage_dice` (Open5e reports it separately).
    pub damage_bonus: Option<i32>,
}

/// One hit in a search response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub slug: String,
    pub name: String,
}

/// A search response as served by Tavernkeep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub kind: String,
    pub query: String,
    /// Total matches upstream (more than `results.len()` when truncated).
    pub count: u64,
    pub results: Vec<SearchHit>,
}

// ---- Upstream (Open5e v1) shapes -------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RawSpell {
    slug: String,
    name: String,
    #[serde(default)]
    spell_level: Option<u8>,
    #[serde(default)]
    level_int: Option<u8>,
    #[serde(default)]
    school: Option<String>,
    #[serde(default)]
    casting_time: Option<String>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    components: Option<String>,
    #[serde(default)]
    requires_concentration: Option<bool>,
    #[serde(default)]
    concentration: Option<String>,
    #[serde(default)]
    can_be_cast_as_ritual: Option<bool>,
    #[serde(default)]
    ritual: Option<String>,
    #[serde(default)]
    desc: Option<String>,
}

fn yes_no(s: Option<&str>) -> Option<bool> {
    match s?.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

impl From<RawSpell> for Spell {
    fn from(r: RawSpell) -> Self {
        Spell {
            slug: r.slug,
            name: r.name,
            level: r.spell_level.or(r.level_int),
            school: r.school,
            casting_time: r.casting_time,
            range: r.range,
            duration: r.duration,
            components: r.components,
            concentration: r
                .requires_concentration
                .or_else(|| yes_no(r.concentration.as_deref())),
            ritual: r
                .can_be_cast_as_ritual
                .or_else(|| yes_no(r.ritual.as_deref())),
            description: r.desc,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawMonster {
    slug: String,
    name: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    cr: Option<f64>,
    #[serde(default)]
    armor_class: Option<u32>,
    #[serde(default)]
    hit_points: Option<u32>,
    #[serde(default, deserialize_with = "numeric_speeds")]
    speed: BTreeMap<String, u32>,
    #[serde(default)]
    strength: Option<u8>,
    #[serde(default)]
    dexterity: Option<u8>,
    #[serde(default)]
    constitution: Option<u8>,
    #[serde(default)]
    intelligence: Option<u8>,
    #[serde(default)]
    wisdom: Option<u8>,
    #[serde(default)]
    charisma: Option<u8>,
    #[serde(default)]
    actions: Option<Vec<RawAction>>,
}

#[derive(Debug, Deserialize)]
struct RawAction {
    name: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    attack_bonus: Option<i32>,
    #[serde(default)]
    damage_dice: Option<String>,
    #[serde(default)]
    damage_bonus: Option<i32>,
}

/// Open5e mixes numeric speeds with flags like `"hover": true`; keep the numbers.
fn numeric_speeds<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, u32>, D::Error> {
    let raw: Option<BTreeMap<String, serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(|n| (k, n))
        })
        .collect())
}

impl From<RawMonster> for Monster {
    fn from(r: RawMonster) -> Self {
        Monster {
            slug: r.slug,
            name: r.name,
            size: r.size,
            kind: r.kind,
            cr: r.cr,
            ac: r.armor_class,
            hp: r.hit_points,
            speed: r.speed,
            abilities: Abilities {
                str: r.strength,
                dex: r.dexterity,
                con: r.constitution,
                int: r.intelligence,
                wis: r.wisdom,
                cha: r.charisma,
            },
            actions: r
                .actions
                .unwrap_or_default()
                .into_iter()
                .map(|a| Action {
                    name: a.name,
                    desc: a.desc.filter(|d| !d.is_empty()),
                    attack_bonus: a.attack_bonus,
                    damage_dice: a.damage_dice.filter(|d| !d.trim().is_empty()),
                    damage_bonus: a.damage_bonus,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawSearchPage {
    #[serde(default)]
    count: Option<u64>,
    results: Vec<RawSearchHit>,
}

#[derive(Debug, Deserialize)]
struct RawSearchHit {
    slug: String,
    name: String,
}

impl RawSearchPage {
    pub fn into_results(self, kind: &str, query: &str) -> SearchResults {
        let results: Vec<SearchHit> = self
            .results
            .into_iter()
            .map(|h| SearchHit {
                slug: h.slug,
                name: h.name,
            })
            .collect();
        SearchResults {
            kind: kind.to_string(),
            query: query.to_string(),
            count: self.count.unwrap_or(results.len() as u64),
            results,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{path}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn maps_fireball() {
        let raw: RawSpell = serde_json::from_str(&fixture("spells/fireball.json")).unwrap();
        let s = Spell::from(raw);
        assert_eq!(s.slug, "fireball");
        assert_eq!(s.level, Some(3));
        assert_eq!(s.school.as_deref(), Some("Evocation"));
        assert_eq!(s.range.as_deref(), Some("150 feet"));
        assert_eq!(s.concentration, Some(false));
        assert_eq!(s.ritual, Some(false));
        assert!(s.description.unwrap().contains("8d6 fire damage"));
    }

    #[test]
    fn maps_every_spell_fixture() {
        for slug in ["fireball", "magic-missile", "cure-wounds", "shield", "wish"] {
            let raw: RawSpell =
                serde_json::from_str(&fixture(&format!("spells/{slug}.json"))).unwrap();
            let s = Spell::from(raw);
            assert_eq!(s.slug, slug);
            assert!(s.level.is_some(), "{slug}");
        }
    }

    #[test]
    fn maps_goblin() {
        let raw: RawMonster = serde_json::from_str(&fixture("monsters/goblin.json")).unwrap();
        let m = Monster::from(raw);
        assert_eq!(m.name, "Goblin");
        assert_eq!(m.kind.as_deref(), Some("Humanoid"));
        assert_eq!(m.cr, Some(0.25));
        assert_eq!(m.ac, Some(15));
        assert_eq!(m.hp, Some(7));
        assert_eq!(m.speed.get("walk"), Some(&30));
        assert_eq!(m.abilities.dex, Some(14));
        assert_eq!(m.actions[0].name, "Scimitar");
        assert_eq!(m.actions[0].attack_bonus, Some(4));
        assert_eq!(m.actions[0].damage_dice.as_deref(), Some("1d6"));
        assert_eq!(m.actions[0].damage_bonus, Some(2));
    }

    #[test]
    fn maps_every_monster_fixture() {
        for slug in ["goblin", "adult-red-dragon", "orc", "zombie", "lich"] {
            let raw: RawMonster =
                serde_json::from_str(&fixture(&format!("monsters/{slug}.json"))).unwrap();
            let m = Monster::from(raw);
            assert_eq!(m.slug, slug);
            assert!(!m.actions.is_empty(), "{slug}");
        }
    }

    #[test]
    fn missing_optional_fields_become_none_and_unknown_are_ignored() {
        let raw: RawMonster = serde_json::from_str(
            r#"{"slug":"blob","name":"Blob","speed":{"walk":10,"hover":true},"actions":null,"mystery":1}"#,
        )
        .unwrap();
        let m = Monster::from(raw);
        assert_eq!(m.ac, None);
        assert_eq!(m.abilities, Abilities::default());
        assert!(m.actions.is_empty());
        assert_eq!(m.speed.len(), 1);

        let raw: RawSpell =
            serde_json::from_str(r#"{"slug":"x","name":"X","concentration":"yes"}"#).unwrap();
        let s = Spell::from(raw);
        assert_eq!(s.level, None);
        assert_eq!(s.concentration, Some(true));
        assert_eq!(s.ritual, None);
    }

    #[test]
    fn maps_search_page() {
        let raw: RawSearchPage = serde_json::from_str(&fixture("search/spells-fire.json")).unwrap();
        let r = raw.into_results("spell", "fire");
        assert_eq!(r.results.len(), 5);
        assert!(r.count >= 5);
        assert_eq!(r.kind, "spell");
    }

    #[test]
    fn monster_serializes_type_field() {
        let raw: RawMonster = serde_json::from_str(&fixture("monsters/orc.json")).unwrap();
        let v = serde_json::to_value(Monster::from(raw)).unwrap();
        assert_eq!(v["type"], "Humanoid");
        assert_eq!(v["abilities"]["str"], 16);
    }
}
