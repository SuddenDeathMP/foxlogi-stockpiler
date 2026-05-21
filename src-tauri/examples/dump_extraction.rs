//! Parses the bundled sample save and writes the extracted, cleaned
//! `PinnedMapToolTipsW` array (with `InitalMapItemDetails` removed) to
//! `data/test_extracted.json` for manual inspection.
//!
//! Run with:  cargo run --example dump_extraction

use std::path::PathBuf;

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let sav = PathBuf::from(manifest).join("../data/76561198012791572_MapData.sav");
    let out = PathBuf::from(manifest).join("../data/test_extracted.json");

    let bytes =
        std::fs::read(&sav).unwrap_or_else(|e| panic!("failed to read {}: {e}", sav.display()));

    let data = foxlogi_stockpiler_lib::extract::extract_pinned_tooltips(&bytes)
        .expect("extraction failed");

    let json = serde_json::to_string_pretty(&data).expect("serialize");
    std::fs::write(&out, format!("{json}\n"))
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out.display()));

    let count = data.as_array().map(Vec::len).unwrap_or(0);
    println!("Wrote {count} pinned-tooltip entries to {}", out.display());
}
