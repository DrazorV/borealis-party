//! Homebrew authoring: an account's own content packs, written as standard Aurora XML and checked by the real engine.
//!
//! Each pack lives in the account's data folder as `party-homebrew/<pack id>.json`: its name, author, version and
//! every element as the Aurora XML `<element>` the author wrote (from a form or by hand). That file is the source; the
//! installed library pack (same id) is made from it through upstream's importer, exactly as if the author had dropped
//! the generated XML files on Additional Content. So what validates here is what loads, and what an index build or
//! export hands out is plain Aurora XML that Aurora and Borealis read unchanged.
//!
//! The server never hosts or publishes an index (the user's D47: copyright risk). [`build`] answers the `.index` and
//! its files as data, for the author's own local agent to write into the author's own repository and push, and the
//! zip export holds the same files. Both take the address of that repository; neither has a default.
//!
//! - Ids never change once an element exists (characters store ids). A new element gets one made from the pack and
//!   its name, unless the author gives one; an id another installed pack already defines is refused.
//! - Every saved change bumps the element's revision, its file's version and the pack's version (the patch number),
//!   so a subscriber's Update sees it.
//! - Validation: the pack with the change is converted and loaded against the account's installed content in a
//!   scratch preview (`preview_import_pack`), which is then dropped. Its findings come back with the file, the
//!   element and the line inside the element's own XML. Unresolved references are errors.
//!
//! Routes (any account, its own packs only) are listed in `party/docs/homebrew-index.md`. The same operations are
//! tools for MCP and the assistant ([`tools`], [`call_tool`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use borealis_lib::commands_content::TauriSink;
use borealis_lib::content_preview::{ImportFileInput, PreviewImportRequest};
use borealis_lib::state::AppState as Backend;
use borealis_migrate::dom::{parse_document, XmlElement, XmlNode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;

use crate::auth::User;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// The folder in an account's data folder that holds its packs.
pub const STORE_DIR: &str = "party-homebrew";
/// Pack ids start with this, so they never meet an imported or bundled pack's id.
pub const PACK_PREFIX: &str = "hb-";
/// Most packs an account keeps.
pub const MAX_PACKS: usize = 20;
/// Most elements in one pack.
pub const MAX_ELEMENTS: usize = 2000;
/// Longest element XML accepted.
pub const MAX_ELEMENT_XML: usize = 256 * 1024;
/// The marker in the installed pack's `pack-meta.json`.
pub const META_MARKER: &str = "partyHomebrew";

// ---------------------------------------------------------------------------------------------------------------
// Element types and their forms.
// ---------------------------------------------------------------------------------------------------------------

/// One setter a type's form offers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldSpec {
    /// The `<set name>`.
    pub setter: &'static str,
    /// What the form calls it.
    pub label: &'static str,
    /// `text`, `number`, `bool`, `select`, `cost` (a number and a currency) or `weight` (pounds).
    pub kind: &'static str,
    /// Choices for `select` (any other value may still be typed in the XML tab).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub options: &'static [&'static str],
    /// The engine refuses the element without it.
    pub required: bool,
    /// For `select` and `text`: the attribute that goes with it, e.g. a weapon's damage `type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr: Option<AttrSpec>,
}

/// An attribute on a setter, with its choices.
#[derive(Debug, Clone, Serialize)]
pub struct AttrSpec {
    /// The attribute name.
    pub name: &'static str,
    /// What the form calls it.
    pub label: &'static str,
    /// Choices (free text when empty).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub options: &'static [&'static str],
}

/// An element type the editor makes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSpec {
    /// The Aurora `type`.
    #[serde(rename = "type")]
    pub ty: &'static str,
    /// The form's title.
    pub label: &'static str,
    /// The file it is written to.
    pub file: &'static str,
    /// The word in a made-up id: `ID_<PACK>_<WORD>_<NAME>`.
    pub id_word: &'static str,
    /// What `<supports>` means for it, or `None` when the form leaves it out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports: Option<&'static str>,
    /// The form shows prerequisite and requirements.
    pub requirements: bool,
    /// The form shows the sheet block (usage, action, sheet text).
    pub sheet: bool,
    /// Setters, in form order.
    pub fields: &'static [FieldSpec],
}

const fn field(setter: &'static str, label: &'static str, kind: &'static str) -> FieldSpec {
    FieldSpec {
        setter,
        label,
        kind,
        options: &[],
        required: false,
        attr: None,
    }
}

const fn required(mut f: FieldSpec) -> FieldSpec {
    f.required = true;
    f
}

const fn select(
    setter: &'static str,
    label: &'static str,
    options: &'static [&'static str],
) -> FieldSpec {
    FieldSpec {
        setter,
        label,
        kind: "select",
        options,
        required: false,
        attr: None,
    }
}

const fn with_attr(mut f: FieldSpec, attr: AttrSpec) -> FieldSpec {
    f.attr = Some(attr);
    f
}

const SCHOOLS: &[&str] = &[
    "Abjuration",
    "Conjuration",
    "Divination",
    "Enchantment",
    "Evocation",
    "Illusion",
    "Necromancy",
    "Transmutation",
];
const LEVELS: &[&str] = &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const RARITIES: &[&str] = &[
    "Common",
    "Uncommon",
    "Rare",
    "Very Rare",
    "Legendary",
    "Artifact",
];
const DAMAGE_TYPES: &[&str] = &[
    "acid",
    "bludgeoning",
    "cold",
    "fire",
    "force",
    "lightning",
    "necrotic",
    "piercing",
    "poison",
    "psychic",
    "radiant",
    "slashing",
    "thunder",
];
const SIZES: &[&str] = &["Tiny", "Small", "Medium", "Large", "Huge", "Gargantuan"];
const COST: FieldSpec = field("cost", "Cost", "cost");
const WEIGHT: FieldSpec = field("weight", "Weight (lb.)", "weight");
const KEYWORDS: FieldSpec = field("keywords", "Keywords", "text");

/// Every type the editor makes, in menu order.
pub const TYPES: &[TypeSpec] = &[
    TypeSpec {
        ty: "Spell",
        label: "Spell",
        file: "spells.xml",
        id_word: "SPELL",
        supports: Some("Spell lists (class names, comma separated)"),
        requirements: false,
        sheet: false,
        fields: &[
            required(select("level", "Level (0 = cantrip)", LEVELS)),
            required(select("school", "School", SCHOOLS)),
            required(field("time", "Casting time", "text")),
            required(field("range", "Range", "text")),
            required(field("duration", "Duration", "text")),
            field("hasVerbalComponent", "Verbal", "bool"),
            field("hasSomaticComponent", "Somatic", "bool"),
            field("hasMaterialComponent", "Material", "bool"),
            field("materialComponent", "Material components", "text"),
            field("isConcentration", "Concentration", "bool"),
            field("isRitual", "Ritual", "bool"),
            KEYWORDS,
        ],
    },
    TypeSpec {
        ty: "Feat",
        label: "Feat",
        file: "feats.xml",
        id_word: "FEAT",
        supports: None,
        requirements: true,
        sheet: true,
        fields: &[KEYWORDS],
    },
    TypeSpec {
        ty: "Item",
        label: "Item",
        file: "items.xml",
        id_word: "ITEM",
        supports: None,
        requirements: false,
        sheet: true,
        fields: &[
            select(
                "category",
                "Category",
                &[
                    "Adventuring Gear",
                    "Tools",
                    "Trade Goods",
                    "Treasure",
                    "Mounts & Vehicles",
                    "Poison",
                ],
            ),
            COST,
            WEIGHT,
            field("stackable", "Stackable", "bool"),
            KEYWORDS,
        ],
    },
    TypeSpec {
        ty: "Magic Item",
        label: "Magic item",
        file: "magic-items.xml",
        id_word: "MAGIC_ITEM",
        supports: None,
        requirements: false,
        sheet: true,
        fields: &[
            select(
                "category",
                "Category",
                &[
                    "Wondrous Items",
                    "Magic Weapons",
                    "Magic Armor",
                    "Potions",
                    "Rings",
                    "Rods",
                    "Scrolls",
                    "Staffs",
                    "Wands",
                ],
            ),
            select(
                "type",
                "Type",
                &[
                    "Wondrous Item",
                    "Weapon",
                    "Armor",
                    "Potion",
                    "Ring",
                    "Rod",
                    "Scroll",
                    "Staff",
                    "Wand",
                ],
            ),
            select("rarity", "Rarity", RARITIES),
            with_attr(
                field("attunement", "Requires attunement", "bool"),
                AttrSpec {
                    name: "addition",
                    label: "Attunement note (e.g. by a wizard)",
                    options: &[],
                },
            ),
            select(
                "slot",
                "Slot",
                &[
                    "misc",
                    "onehand",
                    "twohand",
                    "body",
                    "head",
                    "neck",
                    "ring",
                    "shoulders",
                    "hands",
                    "feet",
                    "waist",
                    "wrists",
                    "eyes",
                ],
            ),
            field("charges", "Charges", "number"),
            COST,
            WEIGHT,
            KEYWORDS,
        ],
    },
    TypeSpec {
        ty: "Weapon",
        label: "Weapon",
        file: "weapons.xml",
        id_word: "WEAPON",
        supports: Some("Weapon properties and groups (ids, comma separated)"),
        requirements: false,
        sheet: false,
        fields: &[
            select("category", "Category", &["Weapons"]),
            with_attr(
                field("damage", "Damage (e.g. 1d8)", "text"),
                AttrSpec {
                    name: "type",
                    label: "Damage type",
                    options: DAMAGE_TYPES,
                },
            ),
            field("versatile", "Versatile damage", "text"),
            field("range", "Range (e.g. 80/320)", "text"),
            select("slot", "Slot", &["onehand", "twohand"]),
            field("proficiency", "Proficiency id", "text"),
            COST,
            WEIGHT,
        ],
    },
    TypeSpec {
        ty: "Armor",
        label: "Armor",
        file: "armor.xml",
        id_word: "ARMOR",
        supports: Some("Armor groups (ids, comma separated)"),
        requirements: false,
        sheet: false,
        fields: &[
            select("category", "Category", &["Armor"]),
            select(
                "armor",
                "Armor type",
                &["Light", "Medium", "Heavy", "Shield"],
            ),
            field("armorClass", "Armor class", "text"),
            field("strength", "Strength needed", "number"),
            select("stealth", "Stealth", &["Disadvantage"]),
            select("slot", "Slot", &["body", "onehand,secondary"]),
            field("proficiency", "Proficiency id", "text"),
            COST,
            WEIGHT,
        ],
    },
    TypeSpec {
        ty: "Race",
        label: "Race",
        file: "races.xml",
        id_word: "RACE",
        supports: None,
        requirements: false,
        sheet: true,
        fields: &[],
    },
    TypeSpec {
        ty: "Sub Race",
        label: "Subrace",
        file: "races.xml",
        id_word: "SUB_RACE",
        supports: Some("Parent race (its name)"),
        requirements: true,
        sheet: true,
        fields: &[],
    },
    TypeSpec {
        ty: "Background",
        label: "Background",
        file: "backgrounds.xml",
        id_word: "BACKGROUND",
        supports: None,
        requirements: false,
        sheet: true,
        fields: &[field("short", "Short description", "text")],
    },
    TypeSpec {
        ty: "Class Feature",
        label: "Class feature",
        file: "class-features.xml",
        id_word: "CLASS_FEATURE",
        supports: Some("Supports (e.g. Eldritch Invocation)"),
        requirements: true,
        sheet: true,
        fields: &[],
    },
    TypeSpec {
        ty: "Archetype",
        label: "Archetype (subclass)",
        file: "archetypes.xml",
        id_word: "ARCHETYPE",
        supports: Some("The class's choice it fills (e.g. Martial Archetype)"),
        requirements: false,
        sheet: true,
        fields: &[],
    },
    TypeSpec {
        ty: "Archetype Feature",
        label: "Archetype feature",
        file: "archetypes.xml",
        id_word: "ARCHETYPE_FEATURE",
        supports: None,
        requirements: true,
        sheet: true,
        fields: &[],
    },
    TypeSpec {
        ty: "Monster",
        label: "Monster",
        file: "monsters.xml",
        id_word: "MONSTER",
        supports: None,
        requirements: false,
        sheet: false,
        fields: &[
            required(select("size", "Size", SIZES)),
            required(field("type", "Creature type", "text")),
            required(field("alignment", "Alignment", "text")),
            required(field("ac", "Armor class", "number")),
            required(field("hp", "Hit points", "number")),
            required(field("hd", "Hit dice (e.g. 3d8+3)", "text")),
            // Aurora's monster parser wants whole feet in each speed.
            field("speed", "Speed (feet)", "number"),
            field("speed:fly", "Fly speed (feet)", "number"),
            field("speed:swim", "Swim speed (feet)", "number"),
            field("speed:climb", "Climb speed (feet)", "number"),
            field("speed:burrow", "Burrow speed (feet)", "number"),
            required(field("str", "Str", "number")),
            required(field("dex", "Dex", "number")),
            required(field("con", "Con", "number")),
            required(field("int", "Int", "number")),
            required(field("wis", "Wis", "number")),
            required(field("cha", "Cha", "number")),
            field("cr", "Challenge rating", "text"),
            field("senses", "Senses", "text"),
            field("languages", "Languages", "text"),
        ],
    },
];

/// The spec of an Aurora type the editor knows.
#[must_use]
pub fn type_spec(ty: &str) -> Option<&'static TypeSpec> {
    TYPES.iter().find(|t| t.ty.eq_ignore_ascii_case(ty))
}

// ---------------------------------------------------------------------------------------------------------------
// The form and its XML.
// ---------------------------------------------------------------------------------------------------------------

/// One `<set>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetterForm {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, String>,
}

/// The `<sheet>` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetForm {
    /// `display="false"` hides the element on the sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub usage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A rule: `<grant>`, `<stat>` or `<select>` with its attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleForm {
    pub kind: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
}

/// An element as the form edits it. Everything the form has no field for is kept in `extraXml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementForm {
    #[serde(rename = "type")]
    pub ty: String,
    /// Made from the pack and the name when empty (new elements only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    /// The pack's name when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Paragraphs separated by blank lines, or XHTML when it starts with `<`.
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub supports: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prerequisite: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requirements: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setters: Vec<SetterForm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<SheetForm>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleForm>,
    /// Other children of the element, as XML (spellcasting, multiclass, compendium...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra_xml: String,
}

fn escape(text: &str, attr: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn attrs_xml(attrs: &[(&str, &str)]) -> String {
    attrs
        .iter()
        .map(|(k, v)| format!(" {k}=\"{}\"", escape(v, true)))
        .collect()
}

/// An attribute name the form may write: letters, digits, `-`, `_` and `:`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().next().is_some_and(char::is_alphabetic)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

/// The description's XHTML: as given when it is markup, else one `<p>` per paragraph.
fn description_xml(text: &str) -> String {
    let text = text.trim();
    if text.starts_with('<') {
        return text.to_string();
    }
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", escape(&p.replace('\n', " "), false)))
        .collect::<Vec<_>>()
        .join("\n\t\t\t")
}

/// The `<element>` XML for a form. The id must be set.
///
/// # Errors
/// What the form lacks, in words for the author.
pub fn form_to_xml(form: &ElementForm) -> Result<String, String> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("Give the element a name.".into());
    }
    if form.ty.trim().is_empty() {
        return Err("Choose the element's type.".into());
    }
    if form.id.trim().is_empty() {
        return Err("The element has no id.".into());
    }
    let mut out = format!(
        "<element{}>\n",
        attrs_xml(&[
            ("name", name),
            ("type", form.ty.trim()),
            ("source", form.source.trim()),
            ("id", form.id.trim()),
        ])
    );
    if !form.supports.trim().is_empty() {
        out.push_str(&format!(
            "\t\t<supports>{}</supports>\n",
            escape(form.supports.trim(), false)
        ));
    }
    if !form.prerequisite.trim().is_empty() {
        out.push_str(&format!(
            "\t\t<prerequisite>{}</prerequisite>\n",
            escape(form.prerequisite.trim(), false)
        ));
    }
    if !form.requirements.trim().is_empty() {
        out.push_str(&format!(
            "\t\t<requirements>{}</requirements>\n",
            escape(form.requirements.trim(), false)
        ));
    }
    out.push_str(&format!(
        "\t\t<description>\n\t\t\t{}\n\t\t</description>\n",
        description_xml(&form.description)
    ));
    if let Some(sheet) = &form.sheet {
        let mut attrs: Vec<(&str, &str)> = Vec::new();
        if sheet.display == Some(false) {
            attrs.push(("display", "false"));
        }
        if !sheet.usage.trim().is_empty() {
            attrs.push(("usage", sheet.usage.trim()));
        }
        if !sheet.action.trim().is_empty() {
            attrs.push(("action", sheet.action.trim()));
        }
        let text = sheet.description.trim();
        if text.is_empty() {
            out.push_str(&format!("\t\t<sheet{} />\n", attrs_xml(&attrs)));
        } else {
            out.push_str(&format!(
                "\t\t<sheet{}>\n\t\t\t<description>{}</description>\n\t\t</sheet>\n",
                attrs_xml(&attrs),
                escape(text, false)
            ));
        }
    }
    let setters: Vec<&SetterForm> = form
        .setters
        .iter()
        .filter(|s| !s.name.trim().is_empty() && !s.value.trim().is_empty())
        .collect();
    if !setters.is_empty() {
        out.push_str("\t\t<setters>\n");
        for s in setters {
            if !valid_name(s.name.trim()) {
                return Err(format!("`{}` is not a setter name.", s.name));
            }
            let mut attrs: Vec<(&str, &str)> = Vec::new();
            for (k, v) in &s.attrs {
                if !valid_name(k) {
                    return Err(format!("`{k}` is not an attribute name."));
                }
                if !v.trim().is_empty() {
                    attrs.push((k.as_str(), v.trim()));
                }
            }
            let value = s.value.trim();
            // Aurora writes a weight as `lb="2"` with "2 lb." for the eye.
            let shown;
            let value = if s.name == "weight" && value.parse::<f64>().is_ok() {
                attrs.retain(|(k, _)| *k != "lb");
                attrs.push(("lb", value));
                shown = format!("{value} lb.");
                shown.as_str()
            } else {
                value
            };
            out.push_str(&format!(
                "\t\t\t<set{}>{}</set>\n",
                attrs_xml(&[&[("name", s.name.trim())], attrs.as_slice()].concat()),
                escape(value, false)
            ));
        }
        out.push_str("\t\t</setters>\n");
    }
    let rules: Vec<&RuleForm> = form
        .rules
        .iter()
        .filter(|r| !r.kind.trim().is_empty())
        .collect();
    if !rules.is_empty() {
        out.push_str("\t\t<rules>\n");
        for r in rules {
            let kind = r.kind.trim();
            if !matches!(kind, "grant" | "stat" | "select") {
                return Err(format!(
                    "`{kind}` is not a rule the form makes (grant, stat or select); write it in the XML tab."
                ));
            }
            let mut attrs: Vec<(&str, &str)> = Vec::new();
            for (k, v) in &r.attrs {
                if !valid_name(k) {
                    return Err(format!("`{k}` is not an attribute name."));
                }
                if !v.trim().is_empty() {
                    attrs.push((k.as_str(), v.trim()));
                }
            }
            out.push_str(&format!("\t\t\t<{kind}{} />\n", attrs_xml(&attrs)));
        }
        out.push_str("\t\t</rules>\n");
    }
    if !form.extra_xml.trim().is_empty() {
        out.push_str("\t\t");
        out.push_str(form.extra_xml.trim());
        out.push('\n');
    }
    out.push_str("\t</element>");
    Ok(out)
}

/// The XML of a node and its children.
fn outer_xml(el: &XmlElement) -> String {
    XmlElement {
        name: String::new(),
        attrs: Vec::new(),
        children: vec![XmlNode::Element(el.clone())],
        line: 0,
    }
    .inner_xml()
}

/// The form for an element's XML. What the form has no field for goes to `extraXml`, so form and XML say the same.
///
/// # Errors
/// As [`parse_element`].
pub fn xml_to_form(xml: &str) -> Result<ElementForm, Vec<Issue>> {
    let parsed = parse_element(xml)?;
    let el = parsed.root;
    let mut form = ElementForm {
        ty: parsed.ty,
        id: parsed.id,
        name: parsed.name,
        source: el.attr("source").unwrap_or_default().to_string(),
        ..ElementForm::default()
    };
    let mut extra = Vec::new();
    for child in el.elements() {
        match child.name.as_str() {
            "supports" => form.supports = child.text().trim().to_string(),
            "prerequisite" => form.prerequisite = child.text().trim().to_string(),
            "requirements" => form.requirements = child.text().trim().to_string(),
            "description" => {
                let paragraphs: Vec<&XmlElement> = child.elements().collect();
                let plain = !paragraphs.is_empty()
                    && paragraphs
                        .iter()
                        .all(|p| p.name == "p" && p.attrs.is_empty() && !p.has_element_children())
                    && child.children.iter().all(|n| match n {
                        XmlNode::Text(t) => t.trim().is_empty(),
                        XmlNode::Element(_) => true,
                        _ => false,
                    });
                form.description = if plain {
                    paragraphs
                        .iter()
                        .map(|p| p.text().trim().to_string())
                        .collect::<Vec<_>>()
                        .join("\n\n")
                } else {
                    child.inner_xml().trim().to_string()
                };
            }
            "sheet" if child.elements().all(|c| c.name == "description") => {
                form.sheet = Some(SheetForm {
                    display: child.attr("display").map(|d| d != "false"),
                    usage: child.attr("usage").unwrap_or_default().to_string(),
                    action: child.attr("action").unwrap_or_default().to_string(),
                    description: child
                        .child("description")
                        .map(|d| d.text().trim().to_string())
                        .unwrap_or_default(),
                });
            }
            "setters" => {
                for set in child.elements() {
                    if set.name != "set" || set.has_element_children() {
                        extra.push(format!("<setters>{}</setters>", outer_xml(set)));
                        continue;
                    }
                    let mut attrs: BTreeMap<String, String> = set
                        .attrs
                        .iter()
                        .filter(|(k, _)| k != "name")
                        .cloned()
                        .collect();
                    let name = set.attr("name").unwrap_or_default().to_string();
                    let mut value = set.text().trim().to_string();
                    if name == "weight" {
                        if let Some(lb) = attrs.remove("lb") {
                            value = lb;
                        }
                    }
                    form.setters.push(SetterForm { name, value, attrs });
                }
            }
            "rules" => {
                for rule in child.elements() {
                    if matches!(rule.name.as_str(), "grant" | "stat" | "select")
                        && !rule.has_element_children()
                    {
                        form.rules.push(RuleForm {
                            kind: rule.name.clone(),
                            attrs: rule.attrs.iter().cloned().collect(),
                        });
                    } else {
                        extra.push(format!("<rules>{}</rules>", outer_xml(rule)));
                    }
                }
            }
            _ => extra.push(outer_xml(child)),
        }
    }
    form.extra_xml = extra.join("\n\t\t");
    Ok(form)
}

/// A finding about homebrew: from the editor's own checks or the engine's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// `error`, `warning` or `info`.
    pub severity: String,
    /// A stable code (the engine's issue code, or the editor's).
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Line in the generated file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Line inside the element's own XML (1 = its `<element>` tag).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_line: Option<u32>,
}

impl Issue {
    fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "error".into(),
            code: code.into(),
            message: message.into(),
            element: None,
            file: None,
            line: None,
            element_line: None,
        }
    }

    fn at(mut self, element: &str, line: Option<u32>) -> Self {
        self.element = Some(element.to_string());
        self.element_line = line;
        self
    }
}

/// An element's XML, read.
pub struct Parsed {
    pub id: String,
    pub ty: String,
    pub name: String,
    pub root: XmlElement,
}

/// An id the editor accepts: `ID_` and then letters, digits, `_` and `-`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    id.len() <= 200
        && id.starts_with("ID_")
        && id.len() > 3
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Read one `<element>`: well-formed, one root, with a name, a type and a usable id.
///
/// # Errors
/// Every problem, with the line in `xml`.
pub fn parse_element(xml: &str) -> Result<Parsed, Vec<Issue>> {
    if xml.len() > MAX_ELEMENT_XML {
        return Err(vec![Issue::error(
            "too_large",
            format!("The element is larger than {} KB.", MAX_ELEMENT_XML / 1024),
        )]);
    }
    let root = parse_document(xml.trim()).map_err(|e| {
        vec![Issue {
            element_line: Some(e.line),
            ..Issue::error("xml_parse", format!("Line {}: {}", e.line, e.message))
        }]
    })?;
    if root.name != "element" {
        return Err(vec![Issue {
            element_line: Some(root.line),
            ..Issue::error(
                "not_an_element",
                format!(
                    "Write one <element>, not <{}>. A whole file goes in through Additional Content.",
                    root.name
                ),
            )
        }]);
    }
    let mut issues = Vec::new();
    let get = |name: &str| {
        root.attr(name)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let (id, ty, name) = (get("id"), get("type"), get("name"));
    if name.is_empty() {
        issues.push(Issue::error(
            "missing_name",
            "The element has no name=\"…\".",
        ));
    }
    if ty.is_empty() {
        issues.push(Issue::error(
            "missing_type",
            "The element has no type=\"…\".",
        ));
    }
    if !valid_id(&id) {
        issues.push(Issue::error(
            "bad_id",
            format!(
                "`{id}` is not an id the editor accepts: it starts with ID_ and has only letters, digits, _ and -."
            ),
        ));
    }
    for issue in &mut issues {
        issue.element_line = Some(root.line);
    }
    if issues.is_empty() {
        Ok(Parsed { id, ty, name, root })
    } else {
        Err(issues)
    }
}

/// `ID_<PACK>_<WORD>_<NAME>` in capitals, from the pack's name and the element's.
#[must_use]
pub fn make_id(pack_name: &str, ty: &str, name: &str) -> String {
    let word = |text: &str| -> String {
        let mut out = String::new();
        for c in text.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_uppercase());
            } else if !out.ends_with('_') {
                out.push('_');
            }
        }
        out.trim_matches('_').to_string()
    };
    let pack: String = word(pack_name).chars().take(24).collect();
    let pack = pack.trim_end_matches('_');
    let ty = type_spec(ty).map_or_else(|| word(ty), |t| t.id_word.to_string());
    let name: String = word(name).chars().take(80).collect();
    [pack, ty.as_str(), name.trim_end_matches('_')]
        .iter()
        .filter(|p| !p.is_empty())
        .fold("ID".to_string(), |acc, p| format!("{acc}_{p}"))
}

// ---------------------------------------------------------------------------------------------------------------
// The store.
// ---------------------------------------------------------------------------------------------------------------

/// One element of a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredElement {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub name: String,
    /// The file it is written to.
    pub file: String,
    /// The `<element>` XML.
    pub xml: String,
    /// Saved changes, from 1.
    pub revision: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// An account's homebrew pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    /// `hb-<slug>`, also the installed pack's id.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub author_url: String,
    /// `major.minor.patch`; every saved change bumps the patch.
    pub version: String,
    /// Each file's own version.
    #[serde(default)]
    pub file_versions: BTreeMap<String, String>,
    pub elements: Vec<StoredElement>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Pack {
    /// The element with this id.
    #[must_use]
    pub fn element(&self, id: &str) -> Option<&StoredElement> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// The files, in a fixed order (by type order, then name), with their elements.
    #[must_use]
    pub fn files(&self) -> Vec<(String, Vec<&StoredElement>)> {
        let mut files: BTreeMap<(usize, String), Vec<&StoredElement>> = BTreeMap::new();
        for e in &self.elements {
            let order = TYPES
                .iter()
                .position(|t| t.file == e.file)
                .unwrap_or(TYPES.len());
            files.entry((order, e.file.clone())).or_default().push(e);
        }
        files.into_iter().map(|((_, f), e)| (f, e)).collect()
    }

    /// The short form for lists.
    #[must_use]
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "description": self.description, "author": self.author,
            "authorUrl": self.author_url, "version": self.version, "elementCount": self.elements.len(),
            "updatedAt": self.updated_at,
        })
    }

    /// The pack with element rows (no XML).
    #[must_use]
    pub fn detail(&self) -> Value {
        let mut out = self.summary();
        out["elements"] = self
            .elements
            .iter()
            .map(|e| {
                json!({ "id": e.id, "type": e.ty, "name": e.name, "file": e.file, "revision": e.revision,
                        "updatedAt": e.updated_at })
            })
            .collect();
        out["files"] = json!(self.file_versions);
        out
    }
}

/// `1.0.4` → `1.0.5`; anything else → `1.0.0`.
#[must_use]
pub fn bump(version: &str) -> String {
    let parts: Vec<u64> = version
        .split('.')
        .map(|p| p.parse::<u64>())
        .collect::<Result<_, _>>()
        .unwrap_or_default();
    match parts.as_slice() {
        [major, minor, patch] => format!("{major}.{minor}.{}", patch + 1),
        [major, minor] => format!("{major}.{minor}.1"),
        _ => "1.0.0".into(),
    }
}

/// A version the author may set: up to three numbers.
fn valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.chars().all(|c| c.is_ascii_digit()))
}

fn store_dir(account_dir: &Path) -> PathBuf {
    account_dir.join(STORE_DIR)
}

fn pack_path(account_dir: &Path, id: &str) -> ApiResult<PathBuf> {
    let ok = id.starts_with(PACK_PREFIX)
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(ApiError::NotFound("There is no such homebrew pack.".into()));
    }
    Ok(store_dir(account_dir).join(format!("{id}.json")))
}

/// Every pack of the account, by name.
///
/// # Errors
/// When the folder can't be read.
pub fn load_packs(account_dir: &Path) -> ApiResult<Vec<Pack>> {
    let dir = store_dir(account_dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ApiError::Internal(format!("{}: {e}", dir.display()))),
    };
    let mut packs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "json") {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Pack>(&t).ok())
            {
                Some(pack) => packs.push(pack),
                None => tracing::warn!("homebrew: cannot read {}", path.display()),
            }
        }
    }
    packs.sort_by_key(|p| p.name.to_lowercase());
    Ok(packs)
}

/// One pack.
///
/// # Errors
/// [`ApiError::NotFound`] when the account has no such pack.
pub fn load_pack(account_dir: &Path, id: &str) -> ApiResult<Pack> {
    let path = pack_path(account_dir, id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|_| ApiError::NotFound("There is no such homebrew pack.".into()))?;
    serde_json::from_str(&text).map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))
}

/// Write a pack (a whole new file, then renamed over the old one).
///
/// # Errors
/// When it can't be written.
pub fn save_pack(account_dir: &Path, pack: &Pack) -> ApiResult<()> {
    let path = pack_path(account_dir, &pack.id)?;
    let io = |e: std::io::Error| ApiError::Internal(format!("{}: {e}", path.display()));
    std::fs::create_dir_all(store_dir(account_dir)).map_err(io)?;
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(pack).map_err(|e| ApiError::Internal(e.to_string()))?;
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(io)
}

fn remove_pack_file(account_dir: &Path, id: &str) -> ApiResult<()> {
    let path = pack_path(account_dir, id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::Internal(format!("{}: {e}", path.display()))),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Files and the index.
// ---------------------------------------------------------------------------------------------------------------

/// One generated file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedFile {
    /// Path relative to the index (`<stem>/<file>.xml`), or the index's own name.
    pub path: String,
    pub text: String,
    /// Where each element's `<element>` tag starts: (id, line).
    #[serde(skip)]
    pub lines: Vec<(String, u32)>,
}

/// The index file's stem for a pack: its id without the prefix, so a renamed pack keeps its file names and address.
#[must_use]
pub fn index_stem(pack: &Pack) -> String {
    pack.id
        .strip_prefix(PACK_PREFIX)
        .unwrap_or(&pack.id)
        .to_string()
}

/// Where a copy hosted nowhere says its files are. Aurora refuses a file whose `<info>` has no `<update>` with a
/// `<file url>`; Borealis never fetches a content file's own address (only an index's), so a reserved name does.
pub const UNHOSTED: &str = "https://homebrew.invalid/";

fn info_xml(name: &str, version: &str, file: &str, url: &str, extra: &str) -> String {
    format!(
        "	<info>
		<name>{}</name>
{extra}		<update version=\"{}\">
			<file name=\"{}\" url=\"{}\" />
		</update>
	</info>
",
        escape(name, false),
        escape(version, true),
        escape(file, true),
        escape(url, true)
    )
}

/// The element files of a pack. With `base` (ending in `/`), each `<info>` names its own address, `base` +
/// `<stem>/<file>`, which is where an index lists it.
#[must_use]
pub fn element_files(pack: &Pack, base: Option<&str>) -> Vec<GeneratedFile> {
    let stem = index_stem(pack);
    pack.files()
        .into_iter()
        .map(|(file, elements)| {
            let path = format!("{stem}/{file}");
            let url = format!("{}{path}", base.unwrap_or(UNHOSTED));
            let version = pack
                .file_versions
                .get(&file)
                .cloned()
                .unwrap_or_else(|| pack.version.clone());
            let label = type_spec(&elements[0].ty).map_or(file.as_str(), |t| t.label);
            let mut text = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<elements>\n");
            text.push_str(&info_xml(
                &format!("{}: {label}", pack.name),
                &version,
                &file,
                &url,
                "",
            ));
            let mut lines = Vec::new();
            for e in elements {
                text.push('\n');
                let line = u32::try_from(text.matches('\n').count() + 1).unwrap_or(u32::MAX);
                lines.push((e.id.clone(), line));
                text.push('\t');
                text.push_str(e.xml.trim());
                text.push('\n');
            }
            text.push_str("</elements>\n");
            GeneratedFile { path, text, lines }
        })
        .collect()
}

/// The `.index` of a pack whose files sit under `base` (ending in `/`), as `<stem>.index` and `<stem>/<file>`.
#[must_use]
pub fn index_file(pack: &Pack, base: &str) -> GeneratedFile {
    let stem = index_stem(pack);
    let name = format!("{stem}.index");
    let mut extra = String::new();
    if !pack.description.trim().is_empty() {
        extra.push_str(&format!(
            "\t\t<description>{}</description>\n",
            escape(pack.description.trim(), false)
        ));
    }
    if !pack.author.trim().is_empty() {
        let url = pack.author_url.trim();
        extra.push_str(&if url.is_empty() {
            format!(
                "\t\t<author>{}</author>\n",
                escape(pack.author.trim(), false)
            )
        } else {
            format!(
                "\t\t<author url=\"{}\">{}</author>\n",
                escape(url, true),
                escape(pack.author.trim(), false)
            )
        });
    }
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<index>\n");
    text.push_str(&info_xml(
        &pack.name,
        &pack.version,
        &name,
        &format!("{base}{name}"),
        &extra,
    ));
    text.push_str("\t<files>\n");
    for (file, _) in pack.files() {
        text.push_str(&format!(
            "\t\t<file name=\"{}\" url=\"{}\" />\n",
            escape(&file, true),
            escape(&format!("{base}{stem}/{file}"), true)
        ));
    }
    text.push_str("\t</files>\n</index>\n");
    GeneratedFile {
        path: name,
        text,
        lines: Vec::new(),
    }
}

/// The index and every element file, for hosting under `base`.
#[must_use]
pub fn index_set(pack: &Pack, base: &str) -> Vec<GeneratedFile> {
    let mut files = vec![index_file(pack, base)];
    files.extend(element_files(pack, Some(base)));
    files
}

/// An address files may be hosted under: `https://`, ending in `/`.
///
/// # Errors
/// In words for the author.
pub fn checked_base(base: &str) -> ApiResult<String> {
    let base = base.trim();
    let parsed = url::Url::parse(base)
        .map_err(|_| ApiError::Invalid("The address must start with https://".into()))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(ApiError::Invalid(
            "The address must start with https://".into(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ApiError::Invalid(
            "The address is a folder: no ? or # in it.".into(),
        ));
    }
    Ok(if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    })
}

/// A zip of the index and its files, as a repository holds them (`<stem>.index`, `<stem>/<file>.xml`).
///
/// # Errors
/// When the zip can't be written.
pub fn zip_of(files: &[GeneratedFile]) -> ApiResult<Vec<u8>> {
    use std::io::Write as _;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let err = |e: &dyn std::fmt::Display| ApiError::Internal(format!("zip: {e}"));
    for f in files {
        zip.start_file(f.path.as_str(), options)
            .map_err(|e| err(&e))?;
        zip.write_all(f.text.as_bytes()).map_err(|e| err(&e))?;
    }
    Ok(zip.finish().map_err(|e| err(&e))?.into_inner())
}

// ---------------------------------------------------------------------------------------------------------------
// The engine.
// ---------------------------------------------------------------------------------------------------------------

/// What the engine said about a pack.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// No errors.
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    /// Elements the engine loaded.
    pub converted: usize,
    pub issues: Vec<Issue>,
}

impl Report {
    fn from_issues(issues: Vec<Issue>, converted: usize) -> Self {
        let count = |s: &str| issues.iter().filter(|i| i.severity == s).count();
        Self {
            ok: count("error") == 0,
            errors: count("error"),
            warnings: count("warning"),
            converted,
            issues,
        }
    }
}

fn backend_error(e: borealis_lib::error::ApiError) -> ApiError {
    serde_json::to_value(&e).map_or_else(|_| ApiError::Internal(e.message), ApiError::Command)
}

/// The files the importer is given: `<pack id>/<file>.xml`, so the set is named and identified as the pack.
fn import_request(pack: &Pack, files: &[GeneratedFile]) -> PreviewImportRequest {
    let stem = index_stem(pack);
    PreviewImportRequest {
        files: files
            .iter()
            .map(|f| ImportFileInput {
                file_name: format!(
                    "{}/{}",
                    pack.id,
                    f.path.strip_prefix(&format!("{stem}/")).unwrap_or(&f.path)
                ),
                content_base64: base64::engine::general_purpose::STANDARD.encode(&f.text),
            })
            .collect(),
        folder: None,
    }
}

/// The element and line of `line` in `file`.
fn locate(files: &[GeneratedFile], file: Option<&str>, line: Option<u32>) -> Option<(String, u32)> {
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let file = files
        .iter()
        .find(|f| Some(base(&f.path)) == file.map(base))?;
    let line = line?;
    file.lines
        .iter()
        .rev()
        .find(|(_, start)| *start <= line)
        .map(|(id, start)| (id.clone(), line - start + 1))
}

/// Convert and load `pack` against the account's installed content, without installing it. With `keep`, the preview
/// stays staged and its id comes back for [`install`].
fn engine_check(backend: &Backend, pack: &Pack, keep: bool) -> ApiResult<(Report, Option<String>)> {
    let files = element_files(pack, None);
    if files.is_empty() {
        return Ok((Report::from_issues(Vec::new(), 0), None));
    }
    let preview = backend
        .preview_import_pack(&import_request(pack, &files))
        .map_err(backend_error)?;
    let mut issues = Vec::new();
    for d in &preview.diagnostics {
        let found = locate(&files, d.file.as_deref(), d.line);
        let element = d
            .element
            .clone()
            .or_else(|| found.as_ref().map(|(id, _)| id.clone()));
        issues.push(Issue {
            severity: d.severity.to_string(),
            code: serde_json::to_value(d.code)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "content".into()),
            message: d.message.clone(),
            element_line: found
                .filter(|(id, _)| Some(id) == element.as_ref())
                .map(|(_, l)| l),
            element,
            file: d.file.clone(),
            line: d.line,
        });
    }
    // A reference to nothing loads but does nothing on a sheet: for the author it is an error.
    for u in &preview.unresolved {
        let by = if u.referenced_by.is_empty() {
            String::new()
        } else {
            format!(" (used by {})", u.referenced_by.join(", "))
        };
        let hint = u.known_source.as_ref().map_or(String::new(), |s| {
            format!(" {s} is installed but has no such id: check the spelling.")
        });
        issues.push(Issue {
            element: u.referenced_by.first().cloned(),
            ..Issue::error(
                "unresolved_reference",
                format!(
                    "`{}` is not an id in this pack or the installed content{by}.{hint}",
                    u.target
                ),
            )
        });
    }
    for c in &preview.conditions {
        issues.push(Issue {
            severity: "warning".into(),
            element: c.referenced_by.first().cloned(),
            ..Issue::error(
                "unknown_condition",
                format!(
                    "A requirement names `{}`, which is not installed here; it counts as absent.",
                    c.target
                ),
            )
        });
    }
    if preview.converted < pack.elements.len() {
        let loaded: BTreeSet<&str> = preview
            .diagnostics
            .iter()
            .filter(|d| d.severity == "error")
            .filter_map(|d| d.element.as_deref())
            .collect();
        if loaded.is_empty() {
            issues.push(Issue::error(
                "not_loaded",
                format!(
                    "Only {} of the pack's {} elements loaded.",
                    preview.converted,
                    pack.elements.len()
                ),
            ));
        }
    }
    let report = Report::from_issues(issues, preview.converted);
    if keep && report.ok {
        return Ok((report, Some(preview.preview_id)));
    }
    let _ = backend.cancel_import_preview(&preview.preview_id);
    Ok((report, None))
}

/// Install a checked pack in the account's library (replacing the earlier copy), or remove it when it is empty.
fn install(backend: &Backend, sink: &TauriSink, pack: &Pack) -> ApiResult<Report> {
    if pack.elements.is_empty() {
        if backend.paths.library().join(&pack.id).exists() {
            backend.remove_pack(&pack.id, sink).map_err(backend_error)?;
        }
        return Ok(Report::from_issues(Vec::new(), 0));
    }
    // Another account's preview may push this one out of upstream's few staged slots: try twice.
    for _ in 0..2 {
        let (report, preview) = engine_check(backend, pack, true)?;
        let Some(preview) = preview else {
            return Ok(report);
        };
        match backend.install_import_preview(&preview, None, sink) {
            Ok(_) => {
                mark_installed(backend, pack);
                return Ok(report);
            }
            Err(e) if e.message.contains("expired") => {}
            Err(e) => return Err(backend_error(e)),
        }
    }
    Err(ApiError::Conflict(
        "The server is busy importing content. Try again.".into(),
    ))
}

/// Mark the installed pack as this account's homebrew (Additional Content shows it as Homebrew).
fn mark_installed(backend: &Backend, pack: &Pack) {
    let path = backend
        .paths
        .library()
        .join(&pack.id)
        .join(borealis_lib::content_packs::PACK_META_FILE);
    let meta = json!({ "kind": "homebrew", META_MARKER: pack.id, "version": pack.version });
    if let Err(e) = std::fs::write(
        &path,
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    ) {
        tracing::warn!("homebrew: {}: {e}", path.display());
    }
}

/// Whether an installed element with this id exists outside `pack`.
fn defined_elsewhere(backend: &Backend, packs: &[Pack], id: &str) -> Option<String> {
    if packs.iter().any(|p| p.element(id).is_some()) {
        return None;
    }
    let found = backend.get_element(id).ok()?;
    let summary = serde_json::to_value(&found.summary).unwrap_or_default();
    Some(format!(
        "{} ({})",
        summary["name"].as_str().unwrap_or(id),
        summary["source"].as_str().unwrap_or("installed content")
    ))
}

// ---------------------------------------------------------------------------------------------------------------
// Operations (the routes and the tools use these).
// ---------------------------------------------------------------------------------------------------------------

/// One account's writes, one at a time.
fn account_lock(account: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: Mutex<Option<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        locks
            .get_or_insert_with(HashMap::new)
            .entry(account.to_string())
            .or_default(),
    )
}

/// An element as the author sends it: a form or the XML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementInput {
    #[serde(default)]
    pub form: Option<ElementForm>,
    #[serde(default)]
    pub xml: Option<String>,
}

/// The refusal for homebrew the engine does not accept: the report rides along.
fn refused(report: &Report, xml: Option<&str>) -> ApiError {
    let first = report
        .issues
        .iter()
        .find(|i| i.severity == "error")
        .map_or_else(String::new, |i| format!(": {}", i.message));
    ApiError::Command(json!({
        "code": "invalid_homebrew",
        "message": format!("The element has {} error(s){first}", report.errors.max(1)),
        "report": report,
        "xml": xml,
    }))
}

fn local_refusal(issues: Vec<Issue>, xml: Option<&str>) -> ApiError {
    refused(&Report::from_issues(issues, 0), xml)
}

/// The element's XML from the input. A new element's form gets an id and the pack's name as its source.
fn element_xml(
    pack: &Pack,
    input: &ElementInput,
    existing: Option<&StoredElement>,
) -> ApiResult<String> {
    match (&input.form, &input.xml) {
        (Some(form), None) => {
            let mut form = form.clone();
            if let Some(e) = existing {
                if !form.id.is_empty() && form.id != e.id {
                    return Err(local_refusal(vec![id_changed(&e.id, &form.id)], None));
                }
                form.id.clone_from(&e.id);
            } else if form.id.trim().is_empty() {
                form.id = make_id(&pack.name, &form.ty, &form.name);
            }
            if form.source.trim().is_empty() {
                form.source.clone_from(&pack.name);
            }
            form_to_xml(&form).map_err(ApiError::Invalid)
        }
        (None, Some(xml)) => Ok(xml.trim().to_string()),
        _ => Err(ApiError::Invalid(
            "Send the element as a form or as XML (one of the two).".into(),
        )),
    }
}

fn id_changed(old: &str, new: &str) -> Issue {
    Issue::error(
        "id_changed",
        format!(
            "The id can't change from {old} to {new}: characters store ids. Make a new element instead."
        ),
    )
}

/// The pack with `xml` put in (a new element, or in place of `replacing`), versions bumped. Checks that need no
/// engine: the XML reads, the id is new (or unchanged), the type is one the editor makes.
fn with_element(
    pack: &Pack,
    xml: &str,
    replacing: Option<&str>,
) -> Result<(Pack, String), Vec<Issue>> {
    let parsed = parse_element(xml)?;
    let Some(spec) = type_spec(&parsed.ty) else {
        let names: Vec<&str> = TYPES.iter().map(|t| t.ty).collect();
        return Err(vec![Issue::error(
            "unknown_type",
            format!(
                "The editor makes {}; `{}` is not one of them.",
                names.join(", "),
                parsed.ty
            ),
        )
        .at(&parsed.id, Some(parsed.root.line))]);
    };
    let mut next = pack.clone();
    let at = now();
    match replacing {
        Some(old) => {
            if parsed.id != old {
                return Err(vec![id_changed(old, &parsed.id).at(old, Some(1))]);
            }
            let Some(e) = next.elements.iter_mut().find(|e| e.id == old) else {
                return Err(vec![Issue::error("not_found", "There is no such element.")]);
            };
            if e.xml.trim() == xml.trim() {
                return Ok((next, parsed.id));
            }
            // A type that moves to another file bumps both files.
            let old_file = std::mem::replace(&mut e.file, spec.file.to_string());
            if old_file != e.file {
                let v = next
                    .file_versions
                    .get(&old_file)
                    .map_or_else(|| "1.0.0".into(), |v| bump(v));
                next.file_versions.insert(old_file, v);
            }
            e.ty = spec.ty.to_string();
            e.name = parsed.name;
            e.xml = xml.trim().to_string();
            e.revision += 1;
            e.updated_at = at;
        }
        None => {
            if next.element(&parsed.id).is_some() {
                return Err(vec![Issue::error(
                    "duplicate_id",
                    format!(
                        "This pack already has an element with the id {}.",
                        parsed.id
                    ),
                )
                .at(&parsed.id, Some(1))]);
            }
            if next.elements.len() >= MAX_ELEMENTS {
                return Err(vec![Issue::error(
                    "too_many",
                    format!("A pack holds at most {MAX_ELEMENTS} elements."),
                )]);
            }
            next.elements.push(StoredElement {
                id: parsed.id.clone(),
                ty: spec.ty.to_string(),
                name: parsed.name,
                file: spec.file.to_string(),
                xml: xml.trim().to_string(),
                revision: 1,
                created_at: at,
                updated_at: at,
            });
        }
    }
    let file = spec.file.to_string();
    let version = next
        .file_versions
        .get(&file)
        .map_or_else(|| "1.0.0".to_string(), |v| bump(v));
    next.file_versions.insert(file, version);
    next.version = bump(&next.version);
    next.updated_at = at;
    Ok((next, parsed.id))
}

/// After a save: the account's open pages reload their content lists.
fn after_change(state: &AppState, user: &User) {
    crate::content_events::changed(&state.hub, &user.id, "packs", "homebrew");
}

/// Every pack of the account.
///
/// # Errors
/// When the store can't be read.
pub async fn list(state: &AppState, user: &User) -> ApiResult<Value> {
    let dir = state.workspaces.dir(&user.id);
    let packs = tokio::task::spawn_blocking(move || load_packs(&dir))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;
    let rows: Vec<Value> = packs.iter().map(Pack::summary).collect();
    Ok(json!({ "packs": rows }))
}

/// One pack, or one of its elements with its XML and form.
///
/// # Errors
/// [`ApiError::NotFound`].
pub async fn get(
    state: &AppState,
    user: &User,
    pack_id: &str,
    element_id: Option<&str>,
) -> ApiResult<Value> {
    let pack = load_pack(&state.workspaces.dir(&user.id), pack_id)?;
    match element_id {
        None => Ok(pack.detail()),
        Some(id) => {
            let e = pack
                .element(id)
                .ok_or_else(|| ApiError::NotFound("There is no such element.".into()))?;
            Ok(element_json(e))
        }
    }
}

fn element_json(e: &StoredElement) -> Value {
    json!({
        "id": e.id, "type": e.ty, "name": e.name, "file": e.file, "revision": e.revision,
        "createdAt": e.created_at, "updatedAt": e.updated_at, "xml": e.xml,
        "form": xml_to_form(&e.xml).ok(),
    })
}

/// The details a pack is made or changed with.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub author_url: Option<String>,
    /// Set by the author, e.g. `2.0.0` for a big change.
    pub version: Option<String>,
}

fn apply_pack_input(pack: &mut Pack, input: &PackInput) -> ApiResult<()> {
    if let Some(name) = &input.name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(ApiError::Invalid(
                "A pack's name has 1 to 80 characters.".into(),
            ));
        }
        pack.name = name.to_string();
    }
    if let Some(d) = &input.description {
        pack.description = d.trim().chars().take(2000).collect();
    }
    if let Some(a) = &input.author {
        pack.author = a.trim().chars().take(80).collect();
    }
    if let Some(u) = &input.author_url {
        let u = u.trim();
        if !u.is_empty() && !u.starts_with("https://") {
            return Err(ApiError::Invalid(
                "The author's address must start with https://".into(),
            ));
        }
        pack.author_url = u.chars().take(300).collect();
    }
    if let Some(v) = &input.version {
        let v = v.trim();
        if !valid_version(v) {
            return Err(ApiError::Invalid(
                "A version is up to three numbers, like 1.2.0.".into(),
            ));
        }
        pack.version = v.to_string();
    }
    Ok(())
}

/// Make a pack.
///
/// # Errors
/// A bad name, too many packs, or a name already used.
pub async fn create_pack(state: &AppState, user: &User, input: &PackInput) -> ApiResult<Value> {
    let lock = account_lock(&user.id);
    let _held = lock.lock().await;
    let dir = state.workspaces.dir(&user.id);
    let packs = load_packs(&dir)?;
    if packs.len() >= MAX_PACKS {
        return Err(ApiError::Invalid(format!(
            "An account keeps at most {MAX_PACKS} homebrew packs."
        )));
    }
    let name = input.name.as_deref().unwrap_or("My Homebrew");
    let id = format!("{PACK_PREFIX}{}", borealis_lib::content_import::slug(name));
    if packs.iter().any(|p| p.id == id) {
        return Err(ApiError::Conflict(format!(
            "You already have a pack called {name}."
        )));
    }
    let at = now();
    let mut pack = Pack {
        id,
        name: name.to_string(),
        description: String::new(),
        author: user.username.clone(),
        author_url: String::new(),
        version: "1.0.0".into(),
        file_versions: BTreeMap::new(),
        elements: Vec::new(),
        created_at: at,
        updated_at: at,
    };
    apply_pack_input(
        &mut pack,
        &PackInput {
            name: input.name.clone().or(Some(name.to_string())),
            ..input.clone()
        },
    )?;
    save_pack(&dir, &pack)?;
    Ok(pack.detail())
}

/// Change a pack's details. Its id (and so its address) stays.
///
/// # Errors
/// [`ApiError::NotFound`], or a bad value.
pub async fn update_pack(
    state: &AppState,
    user: &User,
    pack_id: &str,
    input: &PackInput,
) -> ApiResult<Value> {
    let lock = account_lock(&user.id);
    let _held = lock.lock().await;
    let dir = state.workspaces.dir(&user.id);
    let mut pack = load_pack(&dir, pack_id)?;
    let before = pack.clone();
    apply_pack_input(&mut pack, input)?;
    if pack == before {
        return Ok(pack.detail());
    }
    if input.version.is_none() {
        pack.version = bump(&pack.version);
    }
    pack.updated_at = now();
    save_pack(&dir, &pack)?;
    let workspace = state.workspaces.get(&user.id).await?;
    let installed = pack.clone();
    workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            if backend.paths.library().join(&installed.id).exists() {
                mark_installed(&backend, &installed);
            }
            Ok(())
        })
        .await?;
    after_change(state, user);
    Ok(pack.detail())
}

/// Delete a pack: removed from the library and from the store.
///
/// # Errors
/// [`ApiError::NotFound`].
pub async fn delete_pack(state: &AppState, user: &User, pack_id: &str) -> ApiResult<Value> {
    let lock = account_lock(&user.id);
    let _held = lock.lock().await;
    let dir = state.workspaces.dir(&user.id);
    let pack = load_pack(&dir, pack_id)?;
    let workspace = state.workspaces.get(&user.id).await?;
    let empty = Pack {
        elements: Vec::new(),
        ..pack.clone()
    };
    workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            install(&backend, &TauriSink(handle.clone()), &empty).map(|_| ())
        })
        .await?;
    remove_pack_file(&dir, &pack.id)?;
    crate::content_events::changed(&state.hub, &user.id, "packs", "homebrew");
    Ok(json!({ "deleted": pack.id }))
}

/// The account's pack `pack_id`, or (with none named) its only pack, made as "My Homebrew" when it has none.
async fn target_pack(state: &AppState, user: &User, pack_id: Option<&str>) -> ApiResult<Pack> {
    let dir = state.workspaces.dir(&user.id);
    if let Some(id) = pack_id.filter(|id| !id.is_empty()) {
        return load_pack(&dir, id);
    }
    let mut packs = load_packs(&dir)?;
    match packs.len() {
        0 => {
            let at = now();
            let pack = Pack {
                id: format!("{PACK_PREFIX}my-homebrew"),
                name: "My Homebrew".into(),
                description: String::new(),
                author: user.username.clone(),
                author_url: String::new(),
                version: "1.0.0".into(),
                file_versions: BTreeMap::new(),
                elements: Vec::new(),
                created_at: at,
                updated_at: at,
            };
            save_pack(&dir, &pack)?;
            Ok(pack)
        }
        1 => Ok(packs.remove(0)),
        _ => Err(ApiError::Invalid(
            "You have more than one homebrew pack: say which one.".into(),
        )),
    }
}

/// Check an element (new, or a change to `element_id`) without saving: the editor's checks, then the engine's with
/// the whole pack. With neither form nor XML, the pack as it is.
///
/// # Errors
/// [`ApiError::NotFound`], or input that is neither a form nor XML.
pub async fn validate(
    state: &AppState,
    user: &User,
    pack_id: Option<&str>,
    element_id: Option<&str>,
    input: &ElementInput,
) -> ApiResult<Value> {
    let pack = target_pack(state, user, pack_id).await?;
    let (candidate, xml) = if input.form.is_none() && input.xml.is_none() {
        (pack.clone(), None)
    } else {
        let existing = element_id.and_then(|id| pack.element(id));
        if element_id.is_some() && existing.is_none() {
            return Err(ApiError::NotFound("There is no such element.".into()));
        }
        let xml = element_xml(&pack, input, existing)?;
        match with_element(&pack, &xml, element_id) {
            Ok((next, _)) => (next, Some(xml)),
            Err(issues) => {
                return Ok(json!({ "report": Report::from_issues(issues, 0), "xml": xml }))
            }
        }
    };
    let workspace = state.workspaces.get(&user.id).await?;
    let new_id = xml
        .as_deref()
        .and_then(|x| parse_element(x).ok())
        .map(|p| p.id)
        .filter(|_| element_id.is_none());
    let dir = state.workspaces.dir(&user.id);
    let report = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let mut report = engine_check(&backend, &candidate, false)?.0;
            if let Some(id) = new_id {
                let packs = load_packs(&dir)?;
                if let Some(other) = defined_elsewhere(&backend, &packs, &id) {
                    report.issues.insert(0, taken(&id, &other));
                    report = Report::from_issues(report.issues, report.converted);
                }
            }
            Ok(report)
        })
        .await?;
    Ok(json!({ "report": report, "xml": xml }))
}

fn taken(id: &str, other: &str) -> Issue {
    Issue::error(
        "id_taken",
        format!("{id} is already the id of {other}. Choose another id: one id, one element."),
    )
    .at(id, Some(1))
}

/// Save a new element (`element_id` none) or a change to one, after the engine accepted the whole pack; the library
/// copy is replaced.
///
/// # Errors
/// [`ApiError::Command`] `invalid_homebrew` with the report when the engine refuses it; [`ApiError::NotFound`].
pub async fn save(
    state: &AppState,
    user: &User,
    pack_id: Option<&str>,
    element_id: Option<&str>,
    input: &ElementInput,
) -> ApiResult<Value> {
    let lock = account_lock(&user.id);
    let _held = lock.lock().await;
    let pack = target_pack(state, user, pack_id).await?;
    let existing = element_id.and_then(|id| pack.element(id));
    if element_id.is_some() && existing.is_none() {
        return Err(ApiError::NotFound("There is no such element.".into()));
    }
    let xml = element_xml(&pack, input, existing)?;
    let (next, id) =
        with_element(&pack, &xml, element_id).map_err(|i| local_refusal(i, Some(&xml)))?;
    if next == pack {
        let mut out = element_json(
            pack.element(&id)
                .ok_or_else(|| ApiError::Internal("element".into()))?,
        );
        out["report"] = json!(Report::from_issues(Vec::new(), pack.elements.len()));
        out["pack"] = pack.summary();
        return Ok(out);
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let dir = state.workspaces.dir(&user.id);
    let check = next.clone();
    let new_id = element_id.is_none().then(|| id.clone());
    let report = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            if let Some(id) = new_id {
                let packs = load_packs(&dir)?;
                if let Some(other) = defined_elsewhere(&backend, &packs, &id) {
                    return Ok(Report::from_issues(vec![taken(&id, &other)], 0));
                }
            }
            let report = install(&backend, &TauriSink(handle.clone()), &check)?;
            if report.ok {
                save_pack(&dir, &check)?;
            }
            Ok(report)
        })
        .await?;
    if !report.ok {
        return Err(refused(&report, Some(&xml)));
    }
    after_change(state, user);
    let mut out = element_json(
        next.element(&id)
            .ok_or_else(|| ApiError::Internal("element".into()))?,
    );
    out["report"] = json!(report);
    out["pack"] = next.summary();
    Ok(out)
}

/// Delete an element. Characters that picked it show it as missing, as after any removal.
///
/// # Errors
/// [`ApiError::NotFound`]; the engine refusing what is left (another element of the pack used it).
pub async fn delete_element(
    state: &AppState,
    user: &User,
    pack_id: &str,
    element_id: &str,
) -> ApiResult<Value> {
    let lock = account_lock(&user.id);
    let _held = lock.lock().await;
    let dir = state.workspaces.dir(&user.id);
    let pack = load_pack(&dir, pack_id)?;
    let gone = pack
        .element(element_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("There is no such element.".into()))?;
    let mut next = pack.clone();
    next.elements.retain(|e| e.id != element_id);
    if next.elements.iter().any(|e| e.file == gone.file) {
        let v = next
            .file_versions
            .get(&gone.file)
            .map_or_else(|| "1.0.0".into(), |v| bump(v));
        next.file_versions.insert(gone.file.clone(), v);
    } else {
        next.file_versions.remove(&gone.file);
    }
    next.version = bump(&next.version);
    next.updated_at = now();
    let workspace = state.workspaces.get(&user.id).await?;
    let check = next.clone();
    let report = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let report = install(&backend, &TauriSink(handle.clone()), &check)?;
            if report.ok {
                save_pack(&dir, &check)?;
            }
            Ok(report)
        })
        .await?;
    if !report.ok {
        return Err(refused(&report, None));
    }
    after_change(state, user);
    Ok(json!({ "deleted": element_id, "pack": next.summary() }))
}

/// Why a pack that is not one of the account's homebrew packs can't be built or exported, or `None` when the account
/// has no such pack at all (licences; plan D14: such content is shared as links, never as files).
fn refusal(backend: &Backend, pack_id: &str) -> Option<String> {
    let pack = backend
        .installed_packs()
        .into_iter()
        .find(|p| p.manifest.id == pack_id)?;
    let read = |file: &str| -> Value {
        std::fs::read_to_string(pack.dir.join(file))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    };
    let meta = read(borealis_lib::content_packs::PACK_META_FILE);
    let why = if pack.bundled {
        "comes with Borealis"
    } else if read("import.json")["origin"] == "5etools-import" || pack_id.starts_with("5etools-") {
        "was converted from 5etools data, which may not be shared as files"
    } else if meta["indexUrl"].is_string() || meta["kind"] == "downloaded" {
        "came from an index: share that index's address instead"
    } else {
        "was imported from a file, so as far as the server can tell it is someone else's content"
    };
    Some(format!(
        "{} {why}. Only your own homebrew, made in the homebrew editor, can be built into an index or exported.",
        pack.manifest.name
    ))
}

/// The account's homebrew pack `pack_id` for a build or export; any other pack is refused with the reason.
async fn own_pack(state: &AppState, user: &User, pack_id: Option<&str>) -> ApiResult<Pack> {
    match target_pack(state, user, pack_id).await {
        Err(ApiError::NotFound(m)) => {
            let Some(id) = pack_id.map(str::to_string) else {
                return Err(ApiError::NotFound(m));
            };
            let workspace = state.workspaces.get(&user.id).await?;
            let why = workspace
                .blocking(move |handle| Ok(refusal(&handle.state::<Backend>(), &id)))
                .await?;
            Err(why.map_or(ApiError::NotFound(m), |why| {
                ApiError::Refused("not_redistributable", why)
            }))
        }
        other => other,
    }
}

/// The `.index` and files of the account's own homebrew pack for hosting under `base`: the address of the author's
/// own repository folder (e.g. a GitHub raw address). There is no default, and never this server's address: the
/// server does not host indexes (D47).
///
/// # Errors
/// [`ApiError::Refused`] `not_redistributable` for a pack that is not the account's homebrew, [`ApiError::NotFound`],
/// a missing or bad address, an empty pack.
pub async fn build(
    state: &AppState,
    user: &User,
    pack_id: Option<&str>,
    base: Option<&str>,
) -> ApiResult<(Pack, String, Vec<GeneratedFile>)> {
    let pack = own_pack(state, user, pack_id).await?;
    if pack.elements.is_empty() {
        return Err(ApiError::Invalid("The pack has no elements yet.".into()));
    }
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return Err(ApiError::Invalid(
            "Say where the files will be: the https address of your own repository folder (baseUrl), e.g. https://raw.githubusercontent.com/you/your-homebrew/main/"
                .into(),
        ));
    };
    let base = checked_base(base)?;
    let own = state
        .config
        .public_url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    let host = url::Url::parse(&base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    if own.is_some() && own == host {
        return Err(ApiError::Invalid(
            "This server does not host indexes. Use the address of your own repository.".into(),
        ));
    }
    let files = index_set(&pack, &base);
    Ok((pack, base, files))
}

/// [`build`] as the tools answer it: the files as text for an agent to write, and the same files as a zip a person
/// downloads (`zip.fileName`, `zip.contentBase64`).
///
/// # Errors
/// As [`build`].
pub async fn build_json(
    state: &AppState,
    user: &User,
    pack_id: Option<&str>,
    base: Option<&str>,
) -> ApiResult<Value> {
    let (pack, base, files) = build(state, user, pack_id, base).await?;
    let zip = zip_of(&files)?;
    Ok(json!({
        "packId": pack.id,
        "version": pack.version,
        "base": base,
        "indexUrl": format!("{base}{}", files[0].path),
        "files": files,
        "zip": {
            "fileName": format!("{}.zip", index_stem(&pack)),
            "contentBase64": base64::engine::general_purpose::STANDARD.encode(zip),
        },
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// Routes.
// ---------------------------------------------------------------------------------------------------------------

async fn types_route(_: User) -> Json<Value> {
    Json(json!({ "types": TYPES }))
}

async fn list_route(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    Ok(Json(list(&state, &user).await?))
}

async fn create_pack_route(
    State(state): State<AppState>,
    user: User,
    Json(input): Json<PackInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(create_pack(&state, &user, &input).await?))
}

async fn pack_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(get(&state, &user, &pack, None).await?))
}

async fn update_pack_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
    Json(input): Json<PackInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(update_pack(&state, &user, &pack, &input).await?))
}

async fn delete_pack_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(delete_pack(&state, &user, &pack).await?))
}

async fn element_route(
    State(state): State<AppState>,
    user: User,
    UrlPath((pack, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(get(&state, &user, &pack, Some(&id)).await?))
}

async fn create_element_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
    Json(input): Json<ElementInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(save(&state, &user, Some(&pack), None, &input).await?))
}

async fn update_element_route(
    State(state): State<AppState>,
    user: User,
    UrlPath((pack, id)): UrlPath<(String, String)>,
    Json(input): Json<ElementInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        save(&state, &user, Some(&pack), Some(&id), &input).await?,
    ))
}

async fn delete_element_route(
    State(state): State<AppState>,
    user: User,
    UrlPath((pack, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(delete_element(&state, &user, &pack, &id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateForm {
    pack_id: Option<String>,
    element_id: Option<String>,
    #[serde(flatten)]
    input: ElementInput,
}

async fn validate_route(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<ValidateForm>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        validate(
            &state,
            &user,
            form.pack_id.as_deref(),
            form.element_id.as_deref(),
            &form.input,
        )
        .await?,
    ))
}

/// The XML tab and the form say the same: either one made from the other.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertForm {
    pack_id: Option<String>,
    #[serde(flatten)]
    input: ElementInput,
}

async fn convert_route(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<ConvertForm>,
) -> ApiResult<Json<Value>> {
    match (&form.input.form, &form.input.xml) {
        (None, Some(xml)) => Ok(Json(match xml_to_form(xml) {
            Ok(f) => json!({ "form": f }),
            Err(issues) => json!({ "report": Report::from_issues(issues, 0) }),
        })),
        (Some(_), None) => {
            let pack = target_pack(&state, &user, form.pack_id.as_deref()).await?;
            let existing = form.input.form.as_ref().and_then(|f| pack.element(&f.id));
            Ok(Json(
                json!({ "xml": element_xml(&pack, &form.input, existing)? }),
            ))
        }
        _ => Err(ApiError::Invalid("Send a form or XML.".into())),
    }
}

#[derive(Deserialize)]
struct BaseQuery {
    base: Option<String>,
}

async fn index_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
    Query(q): Query<BaseQuery>,
) -> ApiResult<Json<Value>> {
    let (_, base, files) = build(&state, &user, Some(&pack), q.base.as_deref()).await?;
    Ok(Json(json!({ "base": base, "files": files })))
}

async fn export_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(pack): UrlPath<String>,
    Query(q): Query<BaseQuery>,
) -> ApiResult<Response> {
    let (pack, _, files) = build(&state, &user, Some(&pack), q.base.as_deref()).await?;
    let bytes = zip_of(&files)?;
    let name = format!("{}.zip", index_stem(&pack));
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/homebrew/types", get(types_route))
        .route(
            "/api/homebrew/packs",
            get(list_route).post(create_pack_route),
        )
        .route(
            "/api/homebrew/packs/{pack}",
            get(pack_route)
                .patch(update_pack_route)
                .delete(delete_pack_route),
        )
        .route(
            "/api/homebrew/packs/{pack}/elements",
            post(create_element_route),
        )
        .route(
            "/api/homebrew/packs/{pack}/elements/{id}",
            get(element_route)
                .patch(update_element_route)
                .delete(delete_element_route),
        )
        .route("/api/homebrew/validate", post(validate_route))
        .route("/api/homebrew/convert", post(convert_route))
        .route("/api/homebrew/packs/{pack}/index", get(index_route))
        .route("/api/homebrew/packs/{pack}/export", get(export_route))
}

// ---------------------------------------------------------------------------------------------------------------
// Tools (MCP and the assistant, through party-tools).
// ---------------------------------------------------------------------------------------------------------------

/// The scope a tool needs (part A's `tokens::Scope`; write includes read). At merge, part B's
/// `tools_backend::external_specs()` builds `party_tools::ToolSpec::external(name, title, description, scope,
/// input_schema)` from each of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ToolScope {
    #[serde(rename = "homebrew:read")]
    HomebrewRead,
    #[serde(rename = "homebrew:write")]
    HomebrewWrite,
}

/// A tool: its name, title, the scope a token needs for it, what it does and its input.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub scope: ToolScope,
    pub description: &'static str,
    pub input_schema: Value,
}

fn element_schema(required_id: bool) -> Value {
    json!({
        "type": "object",
        "properties": {
            "packId": { "type": "string", "description": "The pack's id (hb-...). Optional when the account has one pack." },
            "elementId": { "type": "string", "description": "The element's id. Given: a change to it; absent: a new element." },
            "xml": { "type": "string", "description": "One Aurora <element>…</element>." },
            "form": { "type": "object", "description": "The element as fields: type, name, description, supports, prerequisite, requirements, setters [{name,value,attrs}], sheet {usage,action,description,display}, rules [{kind: grant|stat|select, attrs}], extraXml. See homebrew_types." },
        },
        "required": if required_id { json!(["elementId"]) } else { json!([]) },
    })
}

/// The homebrew tools, and `index_build`. There is no publishing tool: the server never hosts an index (D47).
#[must_use]
pub fn tools() -> Vec<ToolSpec> {
    let none = || json!({ "type": "object", "properties": {} });
    vec![
        ToolSpec {
            name: "homebrew_types",
            title: "Homebrew element types",
            scope: ToolScope::HomebrewRead,
            description: "The element types the homebrew editor makes, with their fields (Aurora setters).",
            input_schema: none(),
        },
        ToolSpec {
            name: "homebrew_list",
            title: "List homebrew packs",
            scope: ToolScope::HomebrewRead,
            description: "The account's own homebrew packs, with their versions and element counts.",
            input_schema: none(),
        },
        ToolSpec {
            name: "homebrew_get",
            title: "Read homebrew",
            scope: ToolScope::HomebrewRead,
            description: "A homebrew pack with its element list, or (with elementId) one element's Aurora XML and form.",
            input_schema: json!({ "type": "object", "properties": {
                "packId": { "type": "string" }, "elementId": { "type": "string" } } }),
        },
        ToolSpec {
            name: "homebrew_validate",
            title: "Check homebrew",
            scope: ToolScope::HomebrewRead,
            description: "Check an element (or, with neither xml nor form, the whole pack) with the real engine, without saving. Errors come with their element and line.",
            input_schema: element_schema(false),
        },
        ToolSpec {
            name: "homebrew_create",
            title: "Add homebrew",
            scope: ToolScope::HomebrewWrite,
            description: "Add an element to the account's homebrew pack. The real engine checks it first; errors come back with their element and line and nothing is saved.",
            input_schema: element_schema(false),
        },
        ToolSpec {
            name: "homebrew_update",
            title: "Change homebrew",
            scope: ToolScope::HomebrewWrite,
            description: "Change a homebrew element (its id never changes). Checked by the engine before it is saved; the pack's version goes up.",
            input_schema: element_schema(true),
        },
        ToolSpec {
            name: "homebrew_save",
            title: "Save homebrew",
            scope: ToolScope::HomebrewWrite,
            description: "homebrew_update when elementId is given, else homebrew_create.",
            input_schema: element_schema(false),
        },
        ToolSpec {
            name: "homebrew_delete",
            title: "Delete homebrew",
            scope: ToolScope::HomebrewWrite,
            description: "Delete a homebrew element.",
            input_schema: json!({ "type": "object", "properties": {
                "packId": { "type": "string" }, "elementId": { "type": "string" } }, "required": ["packId", "elementId"] }),
        },
        ToolSpec {
            name: "index_build",
            title: "Build an index",
            scope: ToolScope::HomebrewRead,
            description: "The Aurora .index and element files of the account's own homebrew pack, for hosting in the user's own repository at baseUrl (an https folder, e.g. a GitHub raw address). Write `files` there and push; `zip` is the same for a person to download. This server never hosts them.",
            input_schema: json!({ "type": "object", "properties": {
                "packId": { "type": "string" },
                "baseUrl": { "type": "string", "description": "The https folder the files will be at, ending in /." } },
                "required": ["baseUrl"] }),
        },
    ]
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn input_of(args: &Value) -> ApiResult<ElementInput> {
    Ok(ElementInput {
        form: match args.get("form") {
            None | Some(Value::Null) => None,
            Some(f) => Some(
                serde_json::from_value(f.clone())
                    .map_err(|e| ApiError::Invalid(format!("form: {e}")))?,
            ),
        },
        xml: str_arg(args, "xml").map(str::to_string),
    })
}

/// Run a tool for `user`. The registry has checked the token's scope ([`ToolSpec::scope`]) and stripped `requestId`;
/// every operation works on the user's own packs only.
///
/// # Errors
/// [`ApiError::NotFound`] for an unknown tool, else what the operation answers.
pub async fn call_tool(state: &AppState, user: &User, name: &str, args: Value) -> ApiResult<Value> {
    let pack = str_arg(&args, "packId");
    let element = str_arg(&args, "elementId");
    match name {
        "homebrew_types" => Ok(json!({ "types": TYPES })),
        "homebrew_list" => list(state, user).await,
        "homebrew_get" => {
            let pack = target_pack(state, user, pack).await?;
            get(state, user, &pack.id, element).await
        }
        "homebrew_validate" => validate(state, user, pack, element, &input_of(&args)?).await,
        "homebrew_create" => save(state, user, pack, None, &input_of(&args)?).await,
        "homebrew_update" => {
            let element = element
                .ok_or_else(|| ApiError::Invalid("Say which element (elementId).".into()))?;
            save(state, user, pack, Some(element), &input_of(&args)?).await
        }
        "homebrew_save" => save(state, user, pack, element, &input_of(&args)?).await,
        "homebrew_delete" => {
            let (Some(pack), Some(element)) = (pack, element) else {
                return Err(ApiError::Invalid("Say which pack and element.".into()));
            };
            delete_element(state, user, pack, element).await
        }
        "index_build" => build_json(state, user, pack, str_arg(&args, "baseUrl")).await,
        _ => Err(ApiError::NotFound(format!("There is no tool {name}."))),
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn lantern() -> ElementForm {
        ElementForm {
            ty: "Magic Item".into(),
            id: "ID_TEST_MAGIC_ITEM_LANTERN_OF_TESTING".into(),
            name: "Lantern of Testing".into(),
            source: "Test Pack".into(),
            description: "This lantern sheds light on bugs.\n\nIt has 3 charges & a <handle>."
                .into(),
            setters: vec![
                SetterForm {
                    name: "rarity".into(),
                    value: "Uncommon".into(),
                    ..SetterForm::default()
                },
                SetterForm {
                    name: "weight".into(),
                    value: "2".into(),
                    ..SetterForm::default()
                },
                SetterForm {
                    name: "cost".into(),
                    value: "50".into(),
                    attrs: BTreeMap::from([("currency".into(), "gp".into())]),
                },
            ],
            sheet: Some(SheetForm {
                usage: "3/Long Rest".into(),
                ..SheetForm::default()
            }),
            rules: vec![RuleForm {
                kind: "stat".into(),
                attrs: BTreeMap::from([("name".into(), "ac".into()), ("value".into(), "1".into())]),
            }],
            ..ElementForm::default()
        }
    }

    #[test]
    fn a_form_survives_xml_and_back() {
        let form = lantern();
        let xml = form_to_xml(&form).unwrap();
        assert!(
            xml.contains("<set name=\"weight\" lb=\"2\">2 lb.</set>"),
            "{xml}"
        );
        assert!(xml.contains("3 charges &amp; a &lt;handle&gt;."), "{xml}");
        let back = xml_to_form(&xml).unwrap();
        assert_eq!(back, form);
        assert_eq!(form_to_xml(&back).unwrap(), xml);
    }

    #[test]
    fn unknown_children_stay_as_xml() {
        let xml = r#"<element name="Odd" type="Feat" source="S" id="ID_ODD"><description><p class="flavor">x</p></description><spellcasting name="Odd" ability="Wisdom" /><rules><grant type="Spell" id="ID_X" /><select type="Spell" name="Pick" supports="Wizard" /></rules></element>"#;
        let form = xml_to_form(xml).unwrap();
        assert_eq!(form.description, r#"<p class="flavor">x</p>"#);
        assert!(
            form.extra_xml.contains("<spellcasting"),
            "{}",
            form.extra_xml
        );
        assert_eq!(form.rules.len(), 2);
        let again = xml_to_form(&form_to_xml(&form).unwrap()).unwrap();
        assert_eq!(again, form);
    }

    #[test]
    fn bad_xml_says_where() {
        let issues = xml_to_form(
            "<element name=\"A\" type=\"Feat\" id=\"ID_A\">\n<description>\n</element>",
        )
        .err()
        .unwrap();
        assert_eq!(issues[0].code, "xml_parse");
        assert!(issues[0].element_line.is_some_and(|l| l >= 2), "{issues:?}");
        let issues = parse_element(r#"<element name="A" type="Feat" id="bad id" />"#)
            .err()
            .unwrap();
        assert_eq!(issues[0].code, "bad_id");
        let issues = parse_element("<elements />").err().unwrap();
        assert_eq!(issues[0].code, "not_an_element");
    }

    #[test]
    fn ids_and_versions() {
        assert_eq!(
            make_id("Test Pack", "Magic Item", "Lantern of Testing!"),
            "ID_TEST_PACK_MAGIC_ITEM_LANTERN_OF_TESTING"
        );
        assert!(valid_id(&make_id("ü", "Spell", "ß")));
        assert_eq!(bump("1.0.9"), "1.0.10");
        assert_eq!(bump("2.1"), "2.1.1");
        assert_eq!(bump("x"), "1.0.0");
        assert!(valid_version("2.0.0") && !valid_version("2.0.0.1") && !valid_version("a"));
    }

    fn pack_with(xml: &str) -> Pack {
        let pack = Pack {
            id: "hb-test-pack".into(),
            name: "Test Pack".into(),
            description: "Made up for tests.".into(),
            author: "Tester".into(),
            author_url: String::new(),
            version: "1.0.0".into(),
            file_versions: BTreeMap::new(),
            elements: Vec::new(),
            created_at: 0,
            updated_at: 0,
        };
        with_element(&pack, xml, None).unwrap().0
    }

    #[test]
    fn a_change_bumps_its_file_and_the_pack_and_keeps_the_id() {
        let xml = form_to_xml(&lantern()).unwrap();
        let pack = pack_with(&xml);
        assert_eq!(pack.version, "1.0.1");
        assert_eq!(pack.file_versions["magic-items.xml"], "1.0.0");
        let changed = xml.replace("Uncommon", "Rare");
        let (next, id) = with_element(&pack, &changed, Some(&pack.elements[0].id)).unwrap();
        assert_eq!(id, pack.elements[0].id);
        assert_eq!(next.version, "1.0.2");
        assert_eq!(next.file_versions["magic-items.xml"], "1.0.1");
        assert_eq!(next.elements[0].revision, 2);
        let renamed = changed.replace("ID_TEST_MAGIC_ITEM_LANTERN_OF_TESTING", "ID_OTHER");
        let issues = with_element(&next, &renamed, Some(&pack.elements[0].id))
            .err()
            .unwrap();
        assert_eq!(issues[0].code, "id_changed");
        let issues = with_element(&next, &changed, None).err().unwrap();
        assert_eq!(issues[0].code, "duplicate_id");
        // The same XML again changes nothing.
        let (same, _) = with_element(&next, &changed, Some(&pack.elements[0].id)).unwrap();
        assert_eq!(same, next);
    }

    #[test]
    fn the_index_lists_every_file_with_its_address() {
        let pack = pack_with(&form_to_xml(&lantern()).unwrap());
        let files = index_set(&pack, "https://host.example/i/tok/");
        assert_eq!(files[0].path, "test-pack.index");
        assert_eq!(files[1].path, "test-pack/magic-items.xml");
        assert!(files[0].text.contains(r#"<file name="magic-items.xml" url="https://host.example/i/tok/test-pack/magic-items.xml" />"#));
        assert!(files[0].text.contains(r#"<update version="1.0.1">"#));
        assert!(files[1]
            .text
            .contains(r#"url="https://host.example/i/tok/test-pack/magic-items.xml""#));
        let index = borealis_lib::content_index_aurora::parse_index(
            files[0].text.as_bytes(),
            "https://host.example/i/tok/test-pack.index",
        )
        .unwrap();
        assert_eq!(index.files.len(), 1);
        assert_eq!(index.files[0].name, "magic-items.xml");
        let zip = zip_of(&files).unwrap();
        let names: Vec<String> = borealis_lib::zip_read::read_zip(&zip, |_| true)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["test-pack.index", "test-pack/magic-items.xml"]);
        // The element's own line is found from the file's.
        let file = &element_files(&pack, None)[0];
        let (id, line) = locate(
            std::slice::from_ref(file),
            Some("magic-items.xml"),
            Some(file.lines[0].1 + 2),
        )
        .unwrap();
        assert_eq!(
            (id.as_str(), line),
            ("ID_TEST_MAGIC_ITEM_LANTERN_OF_TESTING", 3)
        );
    }

    #[test]
    fn hosting_addresses_are_https_folders() {
        assert_eq!(
            checked_base("https://raw.githubusercontent.com/o/r/main").unwrap(),
            "https://raw.githubusercontent.com/o/r/main/"
        );
        assert!(checked_base("http://x.example/").is_err());
        assert!(checked_base("https://x.example/?a=1").is_err());
    }
}
