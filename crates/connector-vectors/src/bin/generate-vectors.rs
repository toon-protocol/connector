//! Writes the committed vector set to `vectors/wire-vectors.json`, and the
//! CAIP-19 asset ids to `vectors/asset-ids.json`, at the repository root. Run after any change to the envelope, giftwrap or
//! condition/fulfilment code -- `cargo test -p connector-vectors` is the
//! gate that fails if this was needed and wasn't run (issue #527).

use std::fs;
use std::path::PathBuf;

fn main() {
    let vectors = connector_vectors::generate();
    let json = connector_vectors::to_json(&vectors);

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/wire-vectors.json");
    fs::write(&path, json).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
    println!("wrote {}", path.display());

    // ADR 0086 decision 2: CAIP-19 asset ids, beside the wire vectors (see
    // `AssetIdVectorFile` for why not inside them).
    let assets = connector_vectors::asset_ids_to_json(&connector_vectors::generate_asset_id_file());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/asset-ids.json");
    fs::write(&path, assets).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}
