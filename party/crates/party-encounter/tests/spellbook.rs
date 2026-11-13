//! Monster spellbooks: the golden files for every SRD 5.1 and SRD 5.2.1 caster, the list grammar's edge cases, the
//! 5etools spellcasting shape, the DM's override spells, and the slots and per-day pools seeded into a combatant.

use std::path::{Path, PathBuf};

use borealis_content::{Element, MonsterData};
use party_encounter::combatant::build_creature;
use party_encounter::spellbook::{
    from_5etools, of_monster, parse_text, seed, BookOrigin, Frequency, OverrideSpell, Spellbook,
    SpellsOverride,
};
use serde_json::{json, Value};

fn content(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content")
        .join(dir)
}

/// Every Monster element of a bundled pack, as (id, data).
fn monsters(dir: &str) -> Vec<(String, MonsterData)> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(content(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for p in files {
        let pack: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        for e in pack["elements"].as_array().into_iter().flatten() {
            if e["type"] != "Monster" {
                continue;
            }
            let el: Element = serde_json::from_value(e.clone()).unwrap();
            let data = MonsterData::from_element(&el).unwrap();
            out.push((el.id, data));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// One line per list: "Slots L3x3: counterspell, fireball*" — the golden's compact form.
fn summary(book: &Spellbook) -> Value {
    let lists: Vec<String> = book
        .lists
        .iter()
        .map(|l| {
            let f = match l.frequency {
                Frequency::AtWill => "At will".to_string(),
                Frequency::PerDay { uses, each } => {
                    format!("{uses}/day{}", if each { " each" } else { "" })
                }
                Frequency::Slots { level, slots } => format!("L{level} x{slots}"),
            };
            let names: Vec<String> = l
                .spells
                .iter()
                .map(|s| {
                    let mut n = s.name.clone();
                    if let Some(id) = &s.id {
                        n.push_str(&format!(" [{id}]"));
                    }
                    if let Some(l) = s.fixed_level {
                        n.push_str(&format!(" @{l}"));
                    }
                    if s.baked_in {
                        n.push_str(" (in AC)");
                    }
                    if s.precast {
                        n.push('*');
                    }
                    n
                })
                .collect();
            format!("{f}: {}", names.join(", "))
        })
        .collect();
    json!({
        "name": book.name,
        "origin": book.origin,
        "ability": book.ability,
        "dc": book.save_dc,
        "hit": book.attack_bonus,
        "casterLevel": book.caster_level,
        "class": book.class,
        "lists": lists,
        "chips": book.chips,
    })
}

fn golden(dir: &str, file: &str) {
    let mut out = serde_json::Map::new();
    for (id, data) in monsters(dir) {
        let books = of_monster(&data, None, None);
        if books.is_empty() {
            continue;
        }
        out.insert(id, Value::Array(books.iter().map(summary).collect()));
    }
    let text = serde_json::to_string_pretty(&Value::Object(out)).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(file);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert_eq!(
        want, text,
        "{file} changed: rerun with UPDATE_GOLDEN=1 and review the diff"
    );
}

#[test]
fn golden_srd_5_1_casters() {
    golden("srd-5.1-monsters", "spellbooks-srd-5.1.json");
}

#[test]
fn golden_srd_5_2_1_casters() {
    golden("srd-5.2.1-monsters", "spellbooks-srd-5.2.1.json");
}

/// Every name of every SRD book is a real spell of that edition: no list text is mistaken for a name.
#[test]
fn every_srd_book_name_is_a_spell() {
    for (dir, spells) in [
        ("srd-5.1-monsters", "srd-5.1/spells"),
        ("srd-5.2.1-monsters", "srd-5.2.1/spells"),
    ] {
        let mut names = std::collections::BTreeSet::new();
        for e in std::fs::read_dir(content(spells)).unwrap() {
            let pack: Value =
                serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
            for el in pack["elements"].as_array().unwrap() {
                names.insert(
                    el["name"]
                        .as_str()
                        .unwrap()
                        .to_lowercase()
                        .replace('’', "'"),
                );
            }
        }
        let mut missing = Vec::new();
        for (id, data) in monsters(dir) {
            for b in of_monster(&data, None, None) {
                for (_, s) in b.spells() {
                    let n = s.name.to_lowercase().replace('’', "'");
                    if !names.contains(&n) {
                        missing.push(format!("{id}: {}", s.name));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{dir}: {missing:?}");
    }
}

#[test]
fn list_grammar() {
    let text = "The archmage is an 18th-level spellcaster. Its spellcasting ability is Intelligence (spell save DC 17, \
                +9 to hit with spell attacks). The archmage can cast disguise self and invisibility at will and has \
                the following wizard spells prepared:\nCantrips (at will): fire bolt, light\n1st level (4 slots): \
                detect magic, mage armor,* magic missile\n4th level (3 slots): banishment, stoneskin*\n9th level (1 \
                slot): time stop *The archmage casts these spells on itself before combat.";
    let b = parse_text("Spellcasting", text, None).unwrap();
    assert_eq!(b.caster_level, Some(18));
    assert_eq!(b.save_dc, Some(17));
    assert_eq!(b.attack_bonus, Some(9));
    assert_eq!(b.ability.as_deref(), Some("int"));
    assert_eq!(b.class.as_deref(), Some("wizard"));
    assert_eq!(b.lists[0].frequency, Frequency::AtWill);
    assert_eq!(b.lists[0].spells[0].name, "disguise self");
    assert_eq!(b.lists[0].spells[1].name, "invisibility");
    let precast: Vec<&str> = b
        .spells()
        .filter(|(_, s)| s.precast)
        .map(|(_, s)| s.name.as_str())
        .collect();
    assert_eq!(precast, ["mage armor", "stoneskin", "time stop"]);
    assert_eq!(b.slots().get(&1), Some(&4));
    assert_eq!(b.slots().get(&9), Some(&1));
    assert_eq!(
        b.chips,
        ["The archmage casts these spells on itself before combat."]
    );

    // A trait run into the last list is kept as a chip, not read as a spell.
    let b = parse_text(
        "Innate Spellcasting",
        "The oni's innate spellcasting ability is Charisma (spell save DC 13). The oni can innately cast the \
         following spells, requiring no material components:\nAt will: darkness, invisibility\n1/day each: charm \
         person, gaseous form, sleep Magic Weapons. The oni's weapon attacks are magical.",
        None,
    )
    .unwrap();
    let last = b.lists.last().unwrap();
    assert_eq!(
        last.frequency,
        Frequency::PerDay {
            uses: 1,
            each: true
        }
    );
    assert_eq!(last.spells.last().unwrap().name, "sleep");
    assert_eq!(
        b.chips,
        ["Magic Weapons. The oni's weapon attacks are magical"]
    );

    // A single innately cast spell, its uses from the feature's usage; its DC belongs to the book.
    let b = parse_text(
        "Innate Spellcasting (1/Day)",
        "The mephit can innately cast heat metal (spell save DC 10), requiring no material components. Its innate \
         spellcasting ability is Charisma.",
        Some(&borealis_content::monster_types::Usage::PerDay(1)),
    )
    .unwrap();
    assert_eq!(b.save_dc, Some(10));
    assert_eq!(b.ability.as_deref(), Some("cha"));
    assert_eq!(b.lists.len(), 1);
    assert_eq!(b.lists[0].spells[0].name, "heat metal");
    assert_eq!(b.lists[0].spells[0].note, None);
    assert_eq!(
        b.lists[0].frequency,
        Frequency::PerDay {
            uses: 1,
            each: false
        }
    );

    // A line that isn't a list stays a chip; a star without a "before combat" footnote isn't precast.
    let b = parse_text(
        "Spellcasting",
        "Its spellcasting ability is Wisdom.\n2nd level (2 slots): hold person*\nSomething else entirely",
        None,
    )
    .unwrap();
    assert!(!b.lists[0].spells[0].precast);
    assert_eq!(b.chips, ["Something else entirely"]);

    assert!(parse_text("Multiattack", "Two attacks.", None).is_none());
}

#[test]
fn structured_notes() {
    let (_, dragon) = monsters("srd-5.2.1-monsters")
        .into_iter()
        .find(|(id, _)| id == "ID_SRD521_MONSTER_ADULT_BLACK_DRAGON")
        .unwrap();
    let books = of_monster(&dragon, None, None);
    assert_eq!(books.len(), 1);
    let b = &books[0];
    assert_eq!(b.origin, BookOrigin::Structured);
    assert_eq!(b.section.as_deref(), Some("action"));
    let arrow = b.spells().find(|(_, s)| s.name == "Acid Arrow").unwrap().1;
    assert_eq!(arrow.fixed_level, Some(3));
    assert_eq!(arrow.id.as_deref(), Some("ID_2024_SPELL_ACID_ARROW"));
    assert!(b.chips.is_empty(), "{:?}", b.chips);
}

#[test]
fn five_etools_spellcasting() {
    let raw = json!([{
        "name": "Spellcasting", "type": "spellcasting",
        "headerEntries": ["The adept is a {@i 5th-level} spellcaster. Its spellcasting ability is Wisdom (spell save {@dc 13}, {@hit 5} to hit with spell attacks)."],
        "will": ["{@spell light}"],
        "daily": { "1e": ["{@spell bless}", "{@spell command|XPHB}"], "2": ["{@spell sanctuary}"] },
        "spells": {
            "0": { "spells": ["{@spell sacred flame}"] },
            "1": { "slots": 4, "spells": ["{@spell shield of faith}*"] },
            "3": { "slots": 2, "spells": ["{@spell spirit guardians} (self only)"] }
        },
        "footerEntries": ["*The adept casts this spell on itself before combat."],
        "ability": "wis"
    }]);
    let books = from_5etools(&raw);
    assert_eq!(books.len(), 1);
    let b = &books[0];
    assert_eq!(b.origin, BookOrigin::FiveEtools);
    assert_eq!(
        (b.save_dc, b.attack_bonus, b.caster_level),
        (Some(13), Some(5), Some(5))
    );
    assert_eq!(b.ability.as_deref(), Some("wis"));
    let names: Vec<&str> = b.spells().map(|(_, s)| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "light",
            "sanctuary",
            "bless",
            "command",
            "sacred flame",
            "shield of faith",
            "spirit guardians"
        ]
    );
    let sanctuary = &b.lists[1];
    assert_eq!(
        sanctuary.frequency,
        Frequency::PerDay {
            uses: 2,
            each: false
        }
    );
    assert!(b
        .spells()
        .any(|(_, s)| s.name == "shield of faith" && s.precast));
    assert_eq!(
        b.spells()
            .find(|(_, s)| s.name == "spirit guardians")
            .unwrap()
            .1
            .note
            .as_deref(),
        Some("self only")
    );
    assert_eq!(b.slots().get(&3), Some(&2));
}

#[test]
fn seeds_slots_and_pools() {
    let (_, lich) = monsters("srd-5.1-monsters")
        .into_iter()
        .find(|(id, _)| id == "ID_SRD51_MONSTER_LICH")
        .unwrap();
    let c = build_creature("c1", "Lich", "Undead", Some(&lich), 135);
    let slots: Vec<(u8, u32)> = c.spell_slots.iter().map(|(l, p)| (*l, p.max)).collect();
    assert_eq!(
        slots,
        [
            (1, 4),
            (2, 3),
            (3, 3),
            (4, 3),
            (5, 3),
            (6, 1),
            (7, 1),
            (8, 1),
            (9, 1)
        ]
    );
    assert!(c.spell_slots.values().all(|p| p.remaining == p.max));

    let (_, solar) = monsters("srd-5.1-monsters")
        .into_iter()
        .find(|(id, _)| id == "ID_SRD51_MONSTER_SOLAR")
        .unwrap();
    let c = build_creature("c2", "Solar", "Celestial", Some(&solar), 243);
    assert!(c.spell_slots.is_empty());
    let pools: Vec<(&str, u32)> = c
        .resources
        .iter()
        .filter(|(k, _)| k.starts_with("spell_"))
        .map(|(_, r)| (r.label.as_str(), r.pool.max))
        .collect();
    assert!(pools.contains(&("blade barrier (3/day)", 3)), "{pools:?}");
    assert!(pools.contains(&("commune (1/day)", 1)), "{pools:?}");
    assert_eq!(pools.len(), 5);

    // A shared "1/Day" list is one pool for the whole list.
    let mut c = party_combat::Combatant::creature("c3", "Owl", 19, 12, 2);
    let (_, owl) = monsters("srd-5.2.1-monsters")
        .into_iter()
        .find(|(id, _)| id == "ID_SRD521_MONSTER_GIANT_OWL")
        .unwrap();
    seed(&mut c, &of_monster(&owl, None, None));
    let shared: Vec<_> = c.resources.values().map(|r| r.label.clone()).collect();
    assert_eq!(shared, ["Clairvoyance (1/day)"]);
}

#[test]
fn override_spells() {
    let o = SpellsOverride {
        spells: vec![
            OverrideSpell {
                id: "ID_PHB_SPELL_FIREBALL".into(),
                name: "Fireball".into(),
                frequency: Frequency::Slots { level: 3, slots: 0 },
            },
            OverrideSpell {
                id: "ID_PHB_SPELL_SHIELD".into(),
                name: "Shield".into(),
                frequency: Frequency::PerDay {
                    uses: 2,
                    each: true,
                },
            },
        ],
        slots: [(3, 2), (1, 4)].into_iter().collect(),
        save_dc: Some(15),
        ..SpellsOverride::default()
    };
    o.check().unwrap();
    let b = o.book();
    assert_eq!(b.origin, BookOrigin::Override);
    assert_eq!(b.slots().get(&3), Some(&2));
    assert_eq!(b.slots().get(&1), Some(&4));
    let mut c = party_combat::Combatant::creature("c", "Ogre", 59, 11, -1);
    seed(&mut c, &[b]);
    assert_eq!(c.spell_slots[&3].max, 2);
    assert_eq!(c.resources.len(), 1);

    let mut bad = o.clone();
    bad.slots.insert(10, 1);
    assert!(bad.check().is_err());
    let mut bad = o;
    bad.save_dc = Some(0);
    assert!(bad.check().is_err());
    assert!(serde_json::from_value::<SpellsOverride>(json!({ "spells": [], "extra": 1 })).is_err());
}

#[test]
fn links_through_the_canonical_catalog() {
    use party_spells::canonical::{Candidate, Catalog, Link, Preferences};
    let (_, mage) = monsters("srd-5.1-monsters")
        .into_iter()
        .find(|(id, _)| id == "ID_SRD51_MONSTER_MAGE")
        .unwrap();
    let cand = |id: &str, name: &str, source: &str| Candidate {
        id: id.into(),
        name: name.into(),
        ruleset: "dnd2014".into(),
        source: source.into(),
        level: Some(3),
        date: None,
    };
    let catalog = Catalog::build(
        &[
            cand("ID_PHB_SPELL_FIREBALL", "Fireball", "SRD 5.1"),
            cand("ID_OTHER_FIREBALL", "Fireball", "Another Book"),
        ],
        &Preferences::default(),
    );
    let links =
        party_encounter::spellbook::link(&of_monster(&mage, None, None), &catalog, "dnd2014");
    let fireball = links.iter().find(|l| l.name == "fireball").unwrap();
    assert_eq!(
        fireball.link,
        Link::Linked {
            id: "ID_PHB_SPELL_FIREBALL".into()
        }
    );
    // Names with no installed spell stay listed, unlinked.
    assert!(links
        .iter()
        .any(|l| l.name == "fly" && matches!(l.link, Link::Unlinked { .. })));
    assert_eq!(links.len(), 16);
}

#[test]
fn override_spells_through_overrides() {
    use party_encounter::overrides::{Overrides, OverridesForm, Printed};
    let form: OverridesForm = serde_json::from_value(json!({
        "spells": { "spells": [{ "id": "ID_PHB_SPELL_SHIELD", "name": "Shield",
            "frequency": { "kind": "slots", "level": 1, "slots": 0 } }], "slots": { "1": 3 } }
    }))
    .unwrap();
    let o = Overrides::default()
        .edit(form, &Printed::default())
        .unwrap();
    assert!(o.changes_block());
    let back: Overrides = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
    assert_eq!(back, o);
    let mut c = party_combat::Combatant::creature("c", "Ogre", 59, 11, -1);
    party_encounter::spellbook::seed_overrides(&mut c, None, &o);
    assert_eq!(c.spell_slots[&1].max, 3);

    let bad: OverridesForm =
        serde_json::from_value(json!({ "spells": { "slots": { "1": 12 } } })).unwrap();
    assert!(Overrides::default().edit(bad, &Printed::default()).is_err());
    let drop: OverridesForm = serde_json::from_value(json!({ "spells": null })).unwrap();
    assert_eq!(o.edit(drop, &Printed::default()).unwrap().spells, None);
}

fn find(dir: &str, id: &str) -> MonsterData {
    monsters(dir)
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("{id}"))
}

#[test]
fn spell_abilities_are_linked_with_their_definitions() {
    use party_combat::spell::{MechOrigin, SpellFrequency};
    use party_combat::AbilityKind;
    let mage = build_creature(
        "m",
        "Mage",
        "Humanoid",
        Some(&find("srd-5.1-monsters", "ID_SRD51_MONSTER_MAGE")),
        40,
    );
    let stats = mage.stats.as_ref().unwrap();
    assert_eq!(stats.creature_type.as_deref(), Some("humanoid"));
    assert_eq!(stats.modifier(party_combat::AbilityName::Int), 3);
    assert_eq!(mage.casting.len(), 1);
    assert_eq!(mage.casting[0].dc, Some(14));
    assert_eq!(mage.casting[0].caster_level, Some(9));
    let by_name = |n: &str| mage.abilities.iter().find(|a| a.name == n).unwrap();
    let armor = by_name("mage armor");
    let link = armor.spell.as_ref().unwrap();
    assert_eq!(link.key.as_deref(), Some("ID_PHB_SPELL_MAGE_ARMOR"));
    assert_eq!((link.level, link.frequency), (1, SpellFrequency::Slot));
    let spec = armor.mech.as_ref().unwrap();
    assert_eq!(
        spec.origin,
        MechOrigin::Srd {
            key: "ID_PHB_SPELL_MAGE_ARMOR".into(),
            rev: 1
        }
    );
    assert_eq!(
        (spec.dc, spec.attack, spec.modifier, spec.cantrip_tier),
        (Some(14), Some(6), 3, 2)
    );
    assert_eq!(by_name("shield").kind, AbilityKind::Reaction);
    assert_eq!(
        by_name("fire bolt").spell.as_ref().unwrap().frequency,
        SpellFrequency::AtWill
    );
    // The printed features keep their order ahead of the spells (the sim relies on it).
    assert_eq!(mage.abilities[0].name, "Spellcasting");

    // 2024: "included in AC" is baked in; per-day spells spend their pool.
    let mage = build_creature(
        "m",
        "Mage",
        "Humanoid",
        Some(&find("srd-5.2.1-monsters", "ID_SRD521_MONSTER_MAGE")),
        81,
    );
    let armor = mage
        .abilities
        .iter()
        .find(|a| a.name == "Mage Armor")
        .unwrap();
    assert!(armor.spell.as_ref().unwrap().baked_in);
    assert!(armor.mech.as_ref().unwrap().baked_in);
    let daily = mage
        .abilities
        .iter()
        .find(|a| {
            a.spell
                .as_ref()
                .is_some_and(|l| l.frequency == SpellFrequency::PerDay)
        })
        .unwrap();
    assert!(mage.resources.contains_key(daily.uses.as_ref().unwrap()));
}

/// The curated coverage promise: every spell on an SRD monster stat block, in both editions, links to its SRD
/// element and has a curated definition.
#[test]
fn every_srd_monster_spell_is_curated() {
    let mut missing = Vec::new();
    for dir in ["srd-5.1-monsters", "srd-5.2.1-monsters"] {
        for (id, data) in monsters(dir) {
            let c = build_creature("c", "x", "x", Some(&data), 10);
            for a in c.abilities.iter().filter(|a| a.spell.is_some()) {
                let link = a.spell.as_ref().unwrap();
                if link.key.is_none() || a.mech.is_none() {
                    missing.push(format!("{id}: {}", a.name));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}
