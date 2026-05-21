//! GVAS save parsing and field extraction.
//!
//! Foxhole writes its map data as an Unreal Engine GVAS (`.sav`) file. The
//! relevant payload is the top-level `PinnedMapToolTipsW` array — one entry per
//! pinned stockpile tooltip. We parse the binary save with the [`gvas`] crate,
//! then collapse its verbose, type-tagged JSON into plain values that are
//! pleasant to consume on the server side.
//!
//! The single field `InitalMapItemDetails` is dropped from every array entry
//! (it is the snapshot taken when the pin was first created; the live data
//! lives in `RecentMapItemDetails`).

use std::collections::HashMap;
use std::io::Cursor;

use chrono::SecondsFormat;
use gvas::error::{DeserializeError, Error as GvasError};
use gvas::game_version::GameVersion;
use gvas::GvasFile;
use serde_json::{Map, Value};

/// Top-level GVAS property holding the array we care about.
pub const PINNED_TOOLTIPS_KEY: &str = "PinnedMapToolTipsW";

/// Per-entry field that must be excluded from the extracted payload.
pub const EXCLUDED_FIELD: &str = "InitalMapItemDetails";

/// Enum-typed entry fields whose value carries the full Unreal type as a
/// prefix (e.g. `EWorldConquestMapId::HowlCountyHex`). The prefix is stripped
/// so the payload keeps only the variant (`HowlCountyHex`). Each tuple is
/// `(field name, prefix to strip)`.
pub const STRIPPED_PREFIXES: [(&str, &str); 2] = [
    ("MapId", "EWorldConquestMapId::"),
    ("RenderState", "EPinnedMapWidgetRenderState::"),
];

/// Number of 100ns ticks between 0001-01-01 (UE `FDateTime` epoch) and the
/// Unix epoch (1970-01-01). Used to convert `DateTime` ticks to ISO-8601.
const UE_TICKS_AT_UNIX_EPOCH: i64 = 621_355_968_000_000_000;

/// Keys that describe a property's type rather than its data; skipped when a
/// struct body carries a single anonymous value (e.g. `Vector2D`).
const META_KEYS: [&str; 6] = [
    "type",
    "type_name",
    "field_name",
    "guid",
    "name",
    "enum_type",
];

/// Errors that can occur while parsing a save or extracting its data.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("failed to parse GVAS save: {0}")]
    Gvas(#[from] GvasError),
    #[error("save is missing the `{0}` property")]
    MissingProperty(&'static str),
    #[error("failed to serialize parsed save: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Parse a GVAS `.sav` from raw bytes.
///
/// The `gvas` parser needs a "hint" for any struct whose layout it can't infer
/// from the stream alone. Rather than hard-coding Foxhole's struct names (which
/// would break the moment the game adds a field), we resolve hints lazily: on a
/// [`DeserializeError::MissingHint`] we register a generic struct hint for the
/// reported path and retry. Unknown structs are read as generic property lists,
/// so any non-special hint value works.
pub fn parse_gvas(bytes: &[u8]) -> Result<GvasFile, ExtractError> {
    let mut hints: HashMap<String, String> = HashMap::new();
    loop {
        let mut cursor = Cursor::new(bytes);
        match GvasFile::read_with_hints(&mut cursor, GameVersion::Default, &hints) {
            Ok(file) => return Ok(file),
            Err(GvasError::Deserialize(DeserializeError::MissingHint(_, path, _))) => {
                // Re-inserting an already-present hint would loop forever; the
                // parser only asks again for paths it hasn't seen, so a plain
                // insert is safe and terminates.
                hints.insert(path.to_string(), "Struct".to_string());
            }
            Err(e) => return Err(ExtractError::Gvas(e)),
        }
    }
}

/// Parse a save and return the cleaned `PinnedMapToolTipsW` array, with
/// `InitalMapItemDetails` removed from every entry.
///
/// The returned value is always a JSON array (possibly empty). It is the shape
/// that goes into the `data` field of the API payload.
pub fn extract_pinned_tooltips(bytes: &[u8]) -> Result<Value, ExtractError> {
    let gvas = parse_gvas(bytes)?;
    extract_from_gvas(&gvas)
}

/// Extraction step for an already-parsed save (kept separate so tests can feed
/// a [`GvasFile`] directly).
pub fn extract_from_gvas(gvas: &GvasFile) -> Result<Value, ExtractError> {
    let property = gvas
        .properties
        .0
        .get(PINNED_TOOLTIPS_KEY)
        .ok_or(ExtractError::MissingProperty(PINNED_TOOLTIPS_KEY))?;

    let raw = serde_json::to_value(property)?;
    let simplified = simplify(&raw);

    let mut items = match simplified {
        Value::Array(items) => items,
        // A single entry that didn't deserialize as an array: wrap it so the
        // output shape is stable.
        other => vec![other],
    };

    for item in &mut items {
        if let Value::Object(map) = item {
            map.remove(EXCLUDED_FIELD);
            for (field, prefix) in STRIPPED_PREFIXES {
                if let Some(Value::String(value)) = map.get_mut(field) {
                    if let Some(stripped) = value.strip_prefix(prefix) {
                        *value = stripped.to_string();
                    }
                }
            }
        }
    }

    Ok(Value::Array(items))
}

/// Recursively collapse the gvas JSON representation into plain values.
fn simplify(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get("type") {
                // A tagged `Property` (has a `type` discriminator).
                simplify_property(ty, map)
            } else if let Some(custom) = map.get("CustomStruct") {
                // A `StructPropertyValue::CustomStruct` element (inside an array
                // of structs) — no `type` tag, just the body.
                simplify_struct_body(custom)
            } else if map.len() == 1 {
                // A typed struct value such as `{ "Vector2F": { .. } }` or
                // `{ "DateTime": { "ticks": .. } }`: unwrap the single field.
                let (_, inner) = map.iter().next().expect("len == 1");
                simplify(inner)
            } else {
                // A plain object (e.g. `{ "x": .., "y": .. }`): recurse fields.
                Value::Object(map.iter().map(|(k, v)| (k.clone(), simplify(v))).collect())
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(simplify).collect()),
        scalar => scalar.clone(),
    }
}

/// Simplify a tagged property given its `type` and the surrounding object.
fn simplify_property(ty: &str, map: &Map<String, Value>) -> Value {
    match ty {
        "ArrayProperty" => simplify_array_property(map),
        "StructProperty" => {
            if let Some(custom) = map.get("CustomStruct") {
                return simplify_struct_body(custom);
            }
            if map.get("type_name").and_then(Value::as_str) == Some("DateTime") {
                return datetime_to_iso(map.get("DateTime"));
            }
            // A primitive struct (Vector2D, IntPoint, ...): take its single body
            // field and recurse so e.g. `Vector2F -> { x, y }`.
            map.iter()
                .find(|(k, _)| !META_KEYS.contains(&k.as_str()))
                .map(|(_, v)| simplify(v))
                .unwrap_or(Value::Null)
        }
        // `value`-bearing scalars: Int16, UInt16, Name, Str, Enum, Bool, Float…
        "EnumProperty" => map.get("value").cloned().unwrap_or(Value::Null),
        "ByteProperty" => map
            .get("Byte")
            .or_else(|| map.get("value"))
            .cloned()
            .unwrap_or(Value::Null),
        _ => map.get("value").cloned().unwrap_or(Value::Null),
    }
}

/// Simplify an `ArrayProperty`. The gvas representation is untagged, so the
/// element kind is identified by which content key is present.
fn simplify_array_property(map: &Map<String, Value>) -> Value {
    if let Some(structs) = map.get("structs") {
        return simplify(structs);
    }
    if let Some(properties) = map.get("properties") {
        return simplify(properties);
    }
    // Arrays of primitives are already plain values; pass them through.
    for key in [
        "bools", "enums", "floats", "ints", "names", "strings", "bytes",
    ] {
        if let Some(values) = map.get(key) {
            return values.clone();
        }
    }
    Value::Array(Vec::new())
}

/// Simplify a `CustomStruct` body: a map of `field -> [property, ...]`. In GVAS
/// repeated properties share a name and are grouped into a list; a length-1
/// list (the common case) is collapsed to its single value.
fn simplify_struct_body(body: &Value) -> Value {
    let Value::Object(fields) = body else {
        return simplify(body);
    };
    let mut out = Map::with_capacity(fields.len());
    for (name, value) in fields {
        let simplified = match value {
            Value::Array(list) if list.len() == 1 => simplify(&list[0]),
            Value::Array(list) => Value::Array(list.iter().map(simplify).collect()),
            other => simplify(other),
        };
        out.insert(name.clone(), simplified);
    }
    Value::Object(out)
}

/// Convert an Unreal `FDateTime` (`{ "ticks": i64 }`, 100ns since 0001-01-01)
/// to an ISO-8601 / RFC-3339 UTC string. Falls back to the raw body if the
/// value is out of range or malformed.
fn datetime_to_iso(body: Option<&Value>) -> Value {
    let Some(ticks) = body.and_then(|d| d.get("ticks")).and_then(Value::as_i64) else {
        return body.cloned().unwrap_or(Value::Null);
    };
    let unix_ticks = ticks - UE_TICKS_AT_UNIX_EPOCH;
    let secs = unix_ticks.div_euclid(10_000_000);
    let nanos = (unix_ticks.rem_euclid(10_000_000) * 100) as u32;
    match chrono::DateTime::from_timestamp(secs, nanos) {
        Some(dt) => Value::String(dt.to_rfc3339_opts(SecondsFormat::Millis, true)),
        None => body.cloned().unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = include_bytes!("../../data/76561198012791572_MapData.sav");

    #[test]
    fn sample_save_parses_without_manual_hints() {
        let gvas = parse_gvas(SAMPLE).expect("sample save should parse");
        assert!(gvas.properties.0.contains_key(PINNED_TOOLTIPS_KEY));
    }

    #[test]
    fn extracts_array_of_entries() {
        let extracted = extract_pinned_tooltips(SAMPLE).expect("extraction");
        let items = extracted.as_array().expect("array");
        assert!(!items.is_empty(), "sample save has pinned tooltips");
        // The sample contains six pinned tooltips.
        assert_eq!(items.len(), 6);
    }

    #[test]
    fn excludes_inital_map_item_details_from_every_entry() {
        let extracted = extract_pinned_tooltips(SAMPLE).expect("extraction");
        for item in extracted.as_array().unwrap() {
            let obj = item.as_object().expect("entry is an object");
            assert!(
                !obj.contains_key(EXCLUDED_FIELD),
                "{EXCLUDED_FIELD} must be excluded from every entry"
            );
        }
    }

    #[test]
    fn entries_are_flattened_to_plain_values() {
        let extracted = extract_pinned_tooltips(SAMPLE).expect("extraction");
        let first = &extracted.as_array().unwrap()[0];

        // MapId collapses from an EnumProperty wrapper to a plain string, with
        // the `EWorldConquestMapId::` enum-type prefix stripped.
        assert!(first["MapId"].is_string());
        assert_eq!(first["MapId"], "HowlCountyHex");

        // NormalizedMapCoords collapses to a plain { x, y } object.
        assert!(first["NormalizedMapCoords"]["x"].is_number());
        assert!(first["NormalizedMapCoords"]["y"].is_number());

        // RecentMapItemDetails is retained (only InitalMapItemDetails is dropped).
        assert!(first.get("RecentMapItemDetails").is_some());
    }

    #[test]
    fn enum_type_prefixes_are_stripped_from_every_entry() {
        let extracted = extract_pinned_tooltips(SAMPLE).expect("extraction");
        for item in extracted.as_array().unwrap() {
            for (field, prefix) in STRIPPED_PREFIXES {
                if let Some(value) = item.get(field).and_then(Value::as_str) {
                    assert!(
                        !value.starts_with(prefix),
                        "{field} still has its enum prefix: {value}"
                    );
                }
            }
        }

        // Spot-check concrete stripped values on the first entry.
        let first = &extracted.as_array().unwrap()[0];
        assert_eq!(first["MapId"], "HowlCountyHex");
        assert_eq!(first["RenderState"], "Expanded");
    }
}
