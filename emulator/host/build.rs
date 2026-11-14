use firmware_patch_core::patch_catalog;
use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repository = manifest_dir.join("../..");
    let mut generated = String::new();
    let mut entries = Vec::new();
    for package in patch_catalog() {
        for model in &package.emulator_sd_models {
            let (model_expr, directory) = match model.as_str() {
                "L6" => ("l6fw::Model::L6", "L6"),
                "L6max" => ("l6fw::Model::L6max", "L6max"),
                other => panic!("unsupported model {other} in {}", package.name),
            };
            let asset = repository
                .join("web/public/loadable-patches")
                .join(directory)
                .join(&package.file_name);
            println!("cargo:rerun-if-changed={}", asset.display());
            assert!(
                asset.is_file(),
                "missing emulator SD artifact {}",
                asset.display()
            );
            entries.push(format!(
                "Module {{ patch: {:?}, model: {model_expr}, bytes: include_bytes!({:?}) }},",
                package.name, asset
            ));
        }
    }
    generated.push_str(&format!("const MODULES: [Module; {}] = [", entries.len()));
    generated.push_str(&entries.join("\n"));
    generated.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("sd_modules.rs");
    fs::write(output, generated).expect("write embedded SD module list");
}
