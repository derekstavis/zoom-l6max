//! Embeds every precomputed, Zoom-byte-free template from `templates/`
//! (produced by `firmware-patch-wasm-prebuild`) directly into the compiled
//! binary, as (name, payload bytes, metadata JSON text) triples.
use std::{env, fs, path::Path};

fn main() {
    let templates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    println!("cargo:rerun-if-changed={}", templates_dir.display());
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(templates_dir.join("manifest.json"))
            .expect("read templates/manifest.json"),
    )
    .expect("parse templates/manifest.json");

    let mut generated =
        String::from("pub(crate) static EMBEDDED_UNITS: &[(&str, &[u8], &str)] = &[\n");
    for unit in manifest["units"].as_array().expect("units array") {
        let metadata_file = unit["metadata_file"].as_str().expect("metadata_file");
        let metadata_path = templates_dir
            .join(metadata_file)
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {metadata_file}: {e}"));
        let metadata: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&metadata_path).expect("read metadata file"))
                .expect("parse metadata file");
        let name = metadata["name"].as_str().expect("name");
        let payload_file = metadata["payload_file"].as_str().expect("payload_file");
        let payload_path = templates_dir
            .join(payload_file)
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {payload_file}: {e}"));
        generated.push_str(&format!(
            "    ({name:?}, include_bytes!({payload_path:?}) as &[u8], include_str!({metadata_path:?})),\n"
        ));
    }
    generated.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(
        Path::new(&out_dir).join("templates_generated.rs"),
        generated,
    )
    .expect("write templates_generated.rs");
}
