//! SRD-only fixtures: four level-3 PCs and creatures from the bundled SRD monster packs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use borealis_content::{ContentStore, ContentStoreBuilder};
use party_encounter::creature::Snapshot;
use party_sim::setup::{AgentControl, Control};
use party_sim::{CreatureInput, PcInput, SimOpts, SimSetup};
use serde_json::json;

fn pack(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content")
        .join(name)
}

fn store() -> &'static ContentStore {
    static STORE: OnceLock<ContentStore> = OnceLock::new();
    STORE.get_or_init(|| {
        ContentStoreBuilder::new()
            .add_pack_dir(&pack("srd-5.2.1-monsters"))
            .unwrap()
            .add_pack_dir(&pack("srd-5.1-monsters"))
            .unwrap()
            .build()
    })
}

/// A creature copy by name and ruleset ("dnd2024" / "dnd2014").
pub fn snapshot(name: &str, ruleset: &str) -> Snapshot {
    let e = store()
        .by_type("Monster")
        .find(|s| {
            s.element.name == name
                && s.element.id.contains(if ruleset == "dnd2014" {
                    "SRD51"
                } else {
                    "SRD521"
                })
        })
        .unwrap_or_else(|| panic!("{name} missing"))
        .element
        .clone();
    Snapshot::of_monster(&e).unwrap()
}

pub fn creature(id: &str, name: &str, ruleset: &str, label: &str) -> CreatureInput {
    let snap = snapshot(name, ruleset);
    CreatureInput {
        id: id.into(),
        element_id: format!("el-{name}"),
        snapshot: snap,
        overrides: Default::default(),
        label: label.into(),
        automation: None,
    }
}

fn rec(v: [i32; 6]) -> serde_json::Value {
    json!({ "str": v[0], "dex": v[1], "con": v[2], "int": v[3], "wis": v[4], "cha": v[5] })
}

fn saves(v: [i32; 6]) -> serde_json::Value {
    json!({ "str": {"bonus": v[0]}, "dex": {"bonus": v[1]}, "con": {"bonus": v[2]}, "int": {"bonus": v[3]}, "wis": {"bonus": v[4]}, "cha": {"bonus": v[5]} })
}

/// Level-3 PCs as upstream's snapshot JSON would carry them (only the fields the simulation reads, plus noise).
pub fn pcs() -> Vec<PcInput> {
    let list = json!([
        {
            "id": "pc-fighter", "name": "Bryn", "owner": "acc-1", "ruleset": "dnd2024", "level": {"total": 3},
            "abilities": { "final": rec([16, 12, 14, 10, 12, 8]), "modifier": rec([3, 1, 2, 0, 1, -1]), "save": saves([5, 1, 4, 0, 1, -1]) },
            "stats": { "proficiencyBonus": 2, "armorClass": 18, "initiative": 1, "speed": 30, "features": [{"name": "Second Wind"}, {"name": "Action Surge"}] },
            "attacks": [
                { "id": "a1", "name": "Longsword", "range": "5 ft.", "attackBonus": "+5", "damage": "1d8+3 slashing", "description": "Versatile" },
                { "id": "a2", "name": "Javelin", "range": "30/120 ft.", "attackBonus": "+5", "damage": "1d6+3 piercing", "description": "Thrown" }
            ],
            "play": { "hitPoints": {"max": 28, "current": 28, "temp": 0}, "resources": [{"id": "second-wind", "name": "Second Wind", "max": 2, "current": 2}] }
        },
        {
            "id": "pc-cleric", "name": "Mira", "owner": "acc-2",
            "abilities": { "final": rec([12, 10, 14, 10, 16, 12]), "modifier": rec([1, 0, 2, 0, 3, 1]), "save": saves([1, 0, 2, 0, 5, 3]) },
            "stats": { "proficiencyBonus": 2, "armorClass": 18, "initiative": 0, "speed": 30 },
            "attacks": [
                { "id": "a1", "name": "Mace", "range": "5 ft.", "attackBonus": "+3", "damage": "1d6+1 bludgeoning", "description": "" },
                { "id": "a2", "name": "Sacred Flame", "range": "60 ft.", "attackBonus": "DC 13 DEX", "damage": "1d8 radiant", "description": "Cantrip" }
            ],
            "spellcasting": {
                "classes": [{ "className": "Cleric", "saveDc": 13, "attackBonus": 5, "spells": [
                    {"id": "ID_2024_SPELL_SACRED_FLAME", "name": "Sacred Flame", "level": 0},
                    {"id": "ID_2024_SPELL_HEALING_WORD", "name": "Healing Word", "level": 1, "prepared": true},
                    {"id": "ID_2024_SPELL_CURE_WOUNDS", "name": "Cure Wounds", "level": 1, "prepared": true},
                    {"id": "ID_2024_SPELL_BLESS", "name": "Bless", "level": 1, "prepared": true, "concentration": true}
                ]}],
                "slots": [{"level": 1, "total": 4}, {"level": 2, "total": 2}]
            },
            "play": { "hitPoints": {"max": 24, "current": 24, "temp": 0} }
        },
        {
            "id": "pc-rogue", "name": "Tam", "owner": "acc-3",
            "abilities": { "final": rec([10, 16, 12, 12, 12, 10]), "modifier": rec([0, 3, 1, 1, 1, 0]), "save": saves([0, 5, 1, 3, 1, 0]) },
            "stats": { "proficiencyBonus": 2, "armorClass": 14, "initiative": 3, "speed": 30 },
            "attacks": [
                { "id": "a1", "name": "Shortbow", "range": "80/320 ft.", "attackBonus": "+5", "damage": "1d6+3 piercing", "description": "" },
                { "id": "a2", "name": "Shortsword", "range": "5 ft.", "attackBonus": "+5", "damage": "1d6+3 piercing", "description": "Finesse, Light" }
            ],
            "play": { "hitPoints": {"max": 21, "current": 21, "temp": 0} }
        },
        {
            "id": "pc-wizard", "name": "Ilse", "owner": "acc-4",
            "abilities": { "final": rec([8, 14, 14, 16, 12, 10]), "modifier": rec([-1, 2, 2, 3, 1, 0]), "save": saves([-1, 2, 2, 5, 3, 0]) },
            "stats": { "proficiencyBonus": 2, "armorClass": 12, "initiative": 2, "speed": 30 },
            "attacks": [
                { "id": "a1", "name": "Fire Bolt", "range": "120 ft.", "attackBonus": "+5", "damage": "1d10 fire", "description": "Cantrip" }
            ],
            "spellcasting": {
                "classes": [{ "className": "Wizard", "saveDc": 13, "attackBonus": 5, "spells": [
                    {"id": "ID_2024_SPELL_FIRE_BOLT", "name": "Fire Bolt", "level": 0},
                    {"id": "ID_2024_SPELL_MAGIC_MISSILE", "name": "Magic Missile", "level": 1, "prepared": true},
                    {"id": "ID_2024_SPELL_SHIELD", "name": "Shield", "level": 1, "prepared": true}
                ]}],
                "slots": [{"level": 1, "total": 4}, {"level": 2, "total": 2}]
            },
            "play": { "hitPoints": {"max": 17, "current": 17, "temp": 0} }
        }
    ]);
    serde_json::from_value(list).unwrap()
}

/// 4 PCs vs Ogre + 4 Goblin Warriors (SRD 5.2.1).
pub fn ogre_setup(seed: u32, agent: &str) -> SimSetup {
    let mut creatures = vec![creature("ec-ogre", "Ogre", "dnd2024", "Ogre")];
    for i in 1..=4 {
        creatures.push(creature(
            &format!("ec-gob{i}"),
            "Goblin Warrior",
            "dnd2024",
            &format!("Goblin {i}"),
        ));
    }
    SimSetup {
        edition: party_combat::Edition::Srd521,
        creatures,
        pcs: pcs(),
        start_effects: vec![],
        rules: vec![],
        opts: SimOpts {
            control: Control {
                agent: AgentControl::Side(agent.into()),
            },
            ..SimOpts::default()
        },
        seed,
        spells: Default::default(),
    }
}
