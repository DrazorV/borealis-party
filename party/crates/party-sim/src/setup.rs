//! Inputs of a simulation and how they become its combat: the encounter's creature copies (snapshot, overrides,
//! label), the party's characters (a small serde subset of upstream's character snapshot, or the roster card when
//! the sheet can't be read), the encounter's start effects and rules, and the options.

use std::collections::BTreeMap;

use party_combat::{
    Ability, AbilityKind, AbilityName, Combatant, CombatantId, Condition, DamageClause, EffectSpec,
    EncounterRule, Hp, Pool, Reset, Resource,
};
use party_encounter::creature::Snapshot;
use party_encounter::overrides::Overrides;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One creature of the encounter, as its copy has it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatureInput {
    /// The encounter creature's id (for the fingerprint and the adjudication cache).
    pub id: String,
    /// The element id of the stat block.
    #[serde(default)]
    pub element_id: String,
    pub snapshot: Snapshot,
    #[serde(default)]
    pub overrides: Overrides,
    /// The DM's label ("Goblin 2").
    pub label: String,
}

/// `{str: T, dex: T, ...}` as upstream writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record<T> {
    pub str: T,
    pub dex: T,
    pub con: T,
    pub int: T,
    pub wis: T,
    pub cha: T,
}

impl<T: Clone> Record<T> {
    fn get(&self, a: AbilityName) -> T {
        match a {
            AbilityName::Str => self.str.clone(),
            AbilityName::Dex => self.dex.clone(),
            AbilityName::Con => self.con.clone(),
            AbilityName::Int => self.int.clone(),
            AbilityName::Wis => self.wis.clone(),
            AbilityName::Cha => self.cha.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveEntry {
    pub bonus: i32,
    pub proficient: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PcAbilities {
    #[serde(rename = "final")]
    pub final_score: Option<Record<i32>>,
    pub modifier: Record<i32>,
    pub save: Option<Record<SaveEntry>>,
}

impl PcAbilities {
    #[must_use]
    pub fn score(&self, a: AbilityName) -> Option<i32> {
        self.final_score.as_ref().map(|r| r.get(a))
    }

    #[must_use]
    pub fn save_bonus(&self, a: AbilityName) -> i32 {
        self.save
            .as_ref()
            .map_or_else(|| self.modifier.get(a), |s| s.get(a).bonus)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcStats {
    pub proficiency_bonus: i32,
    pub armor_class: i32,
    pub initiative: i32,
    pub speed: i32,
    pub features: Vec<Value>,
}

/// One attack row of a character sheet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcAttack {
    pub id: String,
    pub name: String,
    pub range: String,
    pub attack_bonus: String,
    pub damage: String,
    pub description: String,
    pub crit_range: u8,
    pub to_hit: Option<i32>,
    pub offhand_damage: Option<String>,
}

impl PcAttack {
    /// A spell or cantrip row (no weapon).
    #[must_use]
    pub fn is_spell(&self) -> bool {
        let d = self.description.to_lowercase();
        d.contains("cantrip") || d.contains("spell")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcSpell {
    pub id: String,
    pub name: String,
    pub level: u32,
    pub prepared: bool,
    pub always_prepared: bool,
    pub concentration: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcClassCasting {
    pub class_name: String,
    pub save_dc: i32,
    pub attack_bonus: i32,
    pub spells: Vec<PcSpell>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotRow {
    pub level: u32,
    pub total: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PcSpellcasting {
    pub classes: Vec<PcClassCasting>,
    pub slots: Vec<SlotRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotUse {
    pub level: u32,
    pub max: u32,
    pub used: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PcResource {
    pub id: String,
    pub name: String,
    pub max: i32,
    pub current: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DefenseEntry {
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcDefenses {
    pub resistances: Vec<DefenseEntry>,
    pub immunities: Vec<DefenseEntry>,
    pub vulnerabilities: Vec<DefenseEntry>,
    pub condition_immunities: Vec<DefenseEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HpView {
    pub max: i32,
    pub current: i32,
    pub temp: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcPlay {
    pub hit_points: HpView,
    pub resources: Vec<PcResource>,
    pub defenses: PcDefenses,
    pub initiative_advantage: bool,
    pub spell_slots: Vec<SlotUse>,
    pub pact_slots: Option<SlotUse>,
}

/// A PC: the part of upstream's `CharacterSnapshot` (camelCase JSON) the simulation reads. Unknown fields are
/// ignored, so the server passes the snapshot's JSON as it is. `card_only` marks a PC built from its roster card
/// (HP, AC, initiative) because its sheet could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PcInput {
    pub id: String,
    pub name: String,
    /// The account that owns it.
    pub owner: String,
    pub abilities: PcAbilities,
    pub stats: PcStats,
    pub attacks: Vec<PcAttack>,
    pub spellcasting: PcSpellcasting,
    pub play: PcPlay,
    pub card_only: bool,
}

impl PcInput {
    /// A PC from its roster card only.
    #[must_use]
    pub fn from_card(
        id: &str,
        owner: &str,
        name: &str,
        hp: Hp,
        ac: i32,
        initiative: i32,
        speed: i32,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            owner: owner.into(),
            stats: PcStats {
                proficiency_bonus: 2,
                armor_class: ac,
                initiative,
                speed,
                features: vec![],
            },
            play: PcPlay {
                hit_points: HpView {
                    max: hp.max,
                    current: hp.current,
                    temp: hp.temp,
                },
                ..PcPlay::default()
            },
            card_only: true,
            ..Self::default()
        }
    }

    /// The names of its features.
    #[must_use]
    pub fn feature_names(&self) -> Vec<String> {
        self.stats
            .features
            .iter()
            .filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_string))
            .collect()
    }

    /// Its prepared or always-prepared leveled spells and its cantrips, once each, in sheet order.
    #[must_use]
    pub fn spells(&self) -> Vec<&PcSpell> {
        let attack_names: Vec<String> =
            self.attacks.iter().map(|a| a.name.to_lowercase()).collect();
        let mut out: Vec<&PcSpell> = Vec::new();
        for c in &self.spellcasting.classes {
            for s in &c.spells {
                let usable = s.level == 0 || s.prepared || s.always_prepared;
                if usable
                    && !attack_names.contains(&s.name.to_lowercase())
                    && !out.iter().any(|o| o.name == s.name)
                {
                    out.push(s);
                }
            }
        }
        out.truncate(12);
        out
    }

    /// A short hash of what the simulation read (report fingerprints).
    #[must_use]
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let text = serde_json::to_string(self).unwrap_or_default();
        let h = Sha256::digest(text.as_bytes());
        h.iter().take(6).map(|b| format!("{b:02x}")).collect()
    }
}

/// Where a side starts, in 30-foot steps from the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Front,
    Near,
    Far,
    Distant,
}

impl Band {
    #[must_use]
    pub fn steps(self) -> i32 {
        match self {
            Band::Front => 0,
            Band::Near => 1,
            Band::Far => 2,
            Band::Distant => 3,
        }
    }

    /// The band of a distance in steps.
    #[must_use]
    pub fn of(steps: i32) -> Self {
        match steps {
            i32::MIN..=0 => Band::Front,
            1 => Band::Near,
            2 => Band::Far,
            _ => Band::Distant,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Start {
    pub party_band: Band,
    pub foes_band: Band,
    /// Combatant ids (`c1`, ...) or encounter creature ids that start surprised.
    pub surprise: Vec<String>,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            party_band: Band::Near,
            foes_band: Band::Near,
            surprise: vec![],
        }
    }
}

/// Who the agent (or the assistant) plays; everyone else plays Basic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentControl {
    /// "foes", "party", "all" or "none".
    Side(String),
    /// Combatant ids.
    Ids(Vec<String>),
}

impl Default for AgentControl {
    fn default() -> Self {
        AgentControl::Side("foes".into())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Control {
    pub agent: AgentControl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcHp {
    #[default]
    Full,
    Current,
}

/// The options of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimOpts {
    pub pc_hp: PcHp,
    pub max_rounds: u32,
    pub start: Start,
    pub lair: bool,
    pub control: Control,
    /// Creature-side cards show health bands, not HP.
    pub fog: bool,
    /// Text-only options can be adjudicated (off in the small profile).
    pub adjudicate: bool,
    /// Steps a run may take.
    pub max_steps: u32,
}

impl Default for SimOpts {
    fn default() -> Self {
        Self {
            pc_hp: PcHp::Full,
            max_rounds: 10,
            start: Start::default(),
            lair: false,
            control: Control::default(),
            fog: true,
            adjudicate: true,
            max_steps: 600,
        }
    }
}

/// Everything a run starts from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSetup {
    pub edition: party_combat::Edition,
    pub creatures: Vec<CreatureInput>,
    pub pcs: Vec<PcInput>,
    /// The encounter's start effects, with targets as encounter creature ids or PC ids.
    #[serde(default)]
    pub start_effects: Vec<EffectSpec>,
    #[serde(default)]
    pub rules: Vec<EncounterRule>,
    #[serde(default)]
    pub opts: SimOpts,
    pub seed: u32,
}

/// Which side a combatant is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Party,
    Foes,
}

/// The engine combatant of a PC: engine-held HP (the simulation applies damage itself), death saves, slots and
/// resources, and one ability per attack row, spell and resource (keys `atk_N`, `spl_N`, `res_N`).
#[must_use]
pub fn pc_combatant(id: &str, pc: &PcInput, hp_mode: PcHp) -> (Combatant, Vec<String>) {
    let hpv = &pc.play.hit_points;
    let max = hpv.max.max(1);
    let current = match hp_mode {
        PcHp::Full => max,
        PcHp::Current => hpv.current.clamp(0, max),
    };
    let mut c = Combatant::character(
        id,
        &pc.owner,
        if pc.name.is_empty() {
            "Character"
        } else {
            &pc.name
        },
        Hp {
            current,
            max,
            temp: if hp_mode == PcHp::Current {
                hpv.temp.max(0)
            } else {
                0
            },
        },
        pc.stats.armor_class.max(1),
        pc.stats.initiative,
    );
    c.hp_authority = party_combat::HpAuthority::Engine;
    c.zero_hp = party_combat::ZeroHpRule::DeathSaves;
    c.controller = party_combat::Controller::Dm;
    c.initiative_advantage = pc.play.initiative_advantage;
    c.speed.walk = u32::try_from(pc.stats.speed).unwrap_or(30).max(5);
    let clause = |list: &[DefenseEntry]| -> Vec<DamageClause> {
        let types: Vec<_> = list
            .iter()
            .filter(|e| e.active)
            .filter_map(|e| crate::sheet::damage_word(&e.name))
            .collect();
        if types.is_empty() {
            vec![]
        } else {
            vec![DamageClause::plain(&types)]
        }
    };
    c.defenses.resistances = clause(&pc.play.defenses.resistances);
    c.defenses.immunities = clause(&pc.play.defenses.immunities);
    c.defenses.vulnerabilities = clause(&pc.play.defenses.vulnerabilities);
    c.defenses.condition_immunities = pc
        .play
        .defenses
        .condition_immunities
        .iter()
        .filter(|e| e.active)
        .filter_map(|e| crate::sheet::condition_word(&e.name))
        .collect::<Vec<Condition>>();
    // Slots: what the tracker shows left (or full), and pact slots on their own level.
    let mut slots: BTreeMap<u8, Pool> = BTreeMap::new();
    if pc.play.spell_slots.is_empty() {
        for s in &pc.spellcasting.slots {
            if s.total > 0 {
                let max = u32::try_from(s.total).unwrap_or(0);
                slots.insert(u8::try_from(s.level).unwrap_or(1), Pool::full(max));
            }
        }
    } else {
        for s in &pc.play.spell_slots {
            if s.max > 0 {
                let remaining = if hp_mode == PcHp::Full {
                    s.max
                } else {
                    s.max.saturating_sub(s.used)
                };
                slots.insert(
                    u8::try_from(s.level).unwrap_or(1),
                    Pool {
                        max: s.max,
                        remaining,
                    },
                );
            }
        }
    }
    if let Some(p) = &pc.play.pact_slots {
        if p.max > 0 {
            let level = u8::try_from(p.level).unwrap_or(1);
            let e = slots.entry(level).or_insert(Pool {
                max: 0,
                remaining: 0,
            });
            let remaining = if hp_mode == PcHp::Full {
                p.max
            } else {
                p.max.saturating_sub(p.used)
            };
            e.max += p.max;
            e.remaining += remaining;
        }
    }
    c.spell_slots = slots;
    let mut keys = Vec::new();
    for (i, a) in pc.attacks.iter().enumerate() {
        let key = format!("atk_{}", i + 1);
        c.abilities.push(Ability {
            key: key.clone(),
            name: a.name.clone(),
            kind: AbilityKind::Action,
            text: String::new(),
            legendary_cost: None,
            recharge: None,
            uses: None,
            group_limit: None,
            requires_effect: None,
            spell: None,
            mech: None,
        });
        keys.push(key);
    }
    for (i, s) in pc.spells().into_iter().enumerate() {
        let key = format!("spl_{}", i + 1);
        let kind = crate::sheet::known_heal(&s.name, party_combat::Edition::Srd521, 0)
            .map_or(AbilityKind::Action, |(_, k)| k);
        c.abilities.push(Ability {
            key: key.clone(),
            name: s.name.clone(),
            kind,
            text: String::new(),
            legendary_cost: None,
            recharge: None,
            uses: None,
            group_limit: None,
            requires_effect: None,
            spell: None,
            mech: None,
        });
        keys.push(key);
    }
    for r in &pc.play.resources {
        let key = format!("res_{}", r.id);
        let max = u32::try_from(r.max.max(0)).unwrap_or(0);
        let remaining = if hp_mode == PcHp::Full {
            max
        } else {
            u32::try_from(r.current.max(0)).unwrap_or(0).min(max)
        };
        c.resources.insert(
            key.clone(),
            Resource {
                label: r.name.clone(),
                pool: Pool { max, remaining },
                reset: Reset::Never,
            },
        );
        c.abilities.push(Ability {
            key: key.clone(),
            name: r.name.clone(),
            kind: AbilityKind::Free,
            text: String::new(),
            legendary_cost: None,
            recharge: None,
            uses: Some(key.clone()),
            group_limit: None,
            requires_effect: None,
            spell: None,
            mech: None,
        });
        keys.push(key);
    }
    (c, keys)
}

/// A combatant id in the simulation: `c1`, `c2`, ... (PCs first, then creatures in encounter order).
#[must_use]
pub fn sim_id(i: usize) -> CombatantId {
    CombatantId::new(format!("c{}", i + 1))
}
