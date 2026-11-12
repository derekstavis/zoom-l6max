use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let patch_root = manifest_dir.join("../../patches");
    println!("cargo:rerun-if-changed={}", patch_root.display());

    let mut packages = Vec::<Value>::new();
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(&patch_root).expect("read patches directory") {
        let entry = entry.expect("read patch directory entry");
        if !entry.file_type().expect("read patch entry type").is_dir() {
            continue;
        }
        let path = entry.path().join("package.toml");
        if !path.is_file() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let text = fs::read_to_string(&path).expect("read package.toml");
        let value: toml::Value = toml::from_str(&text).expect("parse package.toml");
        let package = value
            .get("package")
            .expect("package.toml requires [package]");
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .expect("[package].name must be a string");
        assert_eq!(
            entry.file_name().to_string_lossy(),
            name,
            "package directory and package name differ"
        );
        assert!(
            names.insert(name.to_owned()),
            "duplicate package name {name}"
        );

        let version = package
            .get("version")
            .and_then(toml::Value::as_str)
            .expect("[package].version must be a string");
        assert!(valid_version(version), "invalid package version {version}");
        packages.push(serde_json::to_value(package).expect("serialize package metadata"));
    }

    packages.sort_by(|left, right| {
        let order = |package: &Value| package["order"].as_u64().unwrap_or(u64::MAX);
        order(left)
            .cmp(&order(right))
            .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
    });

    let by_name = packages
        .iter()
        .map(|package| (package["name"].as_str().unwrap(), package))
        .collect::<BTreeMap<_, _>>();
    for package in &packages {
        let name = package["name"].as_str().unwrap();
        for dependency in package["requires"]
            .as_array()
            .expect("requires must be an array")
        {
            let dependency = dependency
                .as_str()
                .expect("dependency must be a package name");
            assert!(
                by_name.contains_key(dependency),
                "package {name} requires missing package {dependency}"
            );
        }
        validate_dependencies(name, &by_name, &mut BTreeSet::new(), &mut BTreeSet::new());
        validate_models(package);
    }
    let file_names = packages
        .iter()
        .map(|package| {
            let name = package["name"].as_str().unwrap();
            composed_file_name(name, &by_name)
        })
        .collect::<Vec<_>>();
    for (package, file_name) in packages.iter_mut().zip(file_names) {
        package
            .as_object_mut()
            .expect("package metadata must be an object")
            .insert("file_name".into(), Value::String(file_name));
    }

    let mut generated = String::new();
    let patch_names = package_names(&packages, "kind", "patch", "firmware_models");
    let wasm_names = package_names(&packages, "kind", "patch", "wasm_models");
    emit_name_array(&mut generated, "PATCHES", &patch_names);
    emit_name_array(&mut generated, "WASM_PATCHES", &wasm_names);

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let catalog_path = out_dir.join("patch_catalog.json");
    fs::write(
        &catalog_path,
        serde_json::to_vec_pretty(&json!({"packages": packages})).unwrap(),
    )
    .expect("write package catalog JSON");
    generated.push_str(&format!(
        "pub const PATCH_CATALOG_JSON: &str = include_str!({:?});\n",
        catalog_path
    ));
    fs::write(out_dir.join("generated_catalog.rs"), generated)
        .expect("write generated Rust package catalog");
}

fn valid_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some(major) = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return false;
    };
    let Some(patch) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && [major, minor, patch]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn package_names(
    packages: &[Value],
    kind_field: &str,
    kind: &str,
    supported_field: &str,
) -> Vec<String> {
    packages
        .iter()
        .filter(|package| {
            package[kind_field] == kind
                && package[supported_field]
                    .as_array()
                    .is_some_and(|models| !models.is_empty())
        })
        .map(|package| package["name"].as_str().unwrap().to_owned())
        .collect()
}

fn emit_name_array(generated: &mut String, name: &str, values: &[String]) {
    generated.push_str(&format!("pub const {name}: [&str; {}] = [", values.len()));
    for value in values {
        generated.push_str(&format!("{value:?},"));
    }
    generated.push_str("];\n");
}

fn validate_models(package: &Value) {
    let name = package["name"].as_str().unwrap();
    let list = |key: &str| {
        package[key]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: {key} must be an array"))
    };
    let models = list("models")
        .iter()
        .map(|model| model.as_str().expect("model names must be strings"))
        .collect::<BTreeSet<_>>();
    for key in [
        "firmware_models",
        "sd_card_models",
        "wasm_models",
        "website_sd_models",
        "emulator_sd_models",
    ] {
        for model in list(key) {
            let model = model
                .as_str()
                .unwrap_or_else(|| panic!("{name}: {key} entries must be strings"));
            assert!(
                models.contains(model),
                "{name}: {key} must be a subset of models"
            );
            assert!(
                model == "L6" || model == "L6max",
                "{name}: unsupported model in {key}"
            );
        }
    }
    assert!(package["name"].is_string());
    assert!(package["title"].is_string());
    assert!(package["summary"].is_string());
    assert!(package["description"].is_string());
    assert!(package["order"].is_number());
    assert!(package["requires"].is_array());
    for key in ["website", "emulator_gui"] {
        assert!(package[key].is_boolean(), "{name}: {key} must be a boolean");
    }
    if package["website"] == true {
        assert!(package["website_order"].is_number());
        assert!(package["readme"].is_string());
    }
    if package["emulator_gui"] == true {
        assert!(package["emulator_order"].is_number());
    }
}

fn validate_dependencies(
    name: &str,
    packages: &BTreeMap<&str, &Value>,
    active: &mut BTreeSet<String>,
    complete: &mut BTreeSet<String>,
) {
    if complete.contains(name) {
        return;
    }
    assert!(
        active.insert(name.to_owned()),
        "package dependency cycle at {name}"
    );
    for dependency in packages[name]["requires"].as_array().unwrap() {
        validate_dependencies(dependency.as_str().unwrap(), packages, active, complete);
    }
    active.remove(name);
    complete.insert(name.to_owned());
}

fn composed_file_name(name: &str, packages: &BTreeMap<&str, &Value>) -> String {
    fn visit(
        name: &str,
        packages: &BTreeMap<&str, &Value>,
        complete: &mut BTreeSet<String>,
        ordered: &mut Vec<String>,
    ) {
        if !complete.insert(name.to_owned()) {
            return;
        }
        for dependency in packages[name]["requires"].as_array().unwrap() {
            visit(dependency.as_str().unwrap(), packages, complete, ordered);
        }
        ordered.push(name.to_owned());
    }

    let mut ordered = Vec::new();
    let mut complete = BTreeSet::new();
    visit(name, packages, &mut complete, &mut ordered);
    let mut members = vec![name.to_owned()];
    members.extend(ordered.into_iter().filter(|member| member != name));
    let stems = members
        .iter()
        .map(|member| {
            let package = packages[member.as_str()];
            format!(
                "{}-{}",
                package["name"].as_str().unwrap(),
                package["version"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    format!("{}.bin", stems.join("-"))
}
