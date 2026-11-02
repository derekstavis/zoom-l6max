//! The local `config.toml`, which holds paths that stay on one machine: the working
//! directory's for a run from the repository, or the app's data directory's when
//! bundled. The emulator reads all of it and the GUI saves its selections here. The
//! test runner reads only the firmware, and each scenario declares its own inputs.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const FILE_NAME: &str = "config.toml";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    /// The model to run without `--model`.
    pub model: Option<String>,
    /// The stock update package of each model.
    #[serde(default)]
    pub firmware: BTreeMap<String, PathBuf>,
    #[serde(default)]
    emulator: BTreeMap<String, Emulator>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Emulator {
    sd_card: Option<PathBuf>,
    /// WAV files by input channel.
    #[serde(default)]
    input_patch: BTreeMap<String, PathBuf>,
}

pub fn model_name(model: l6fw::Model) -> &'static str {
    match model {
        l6fw::Model::L6 => "l6",
        l6fw::Model::L6max => "l6max",
    }
}

pub fn parse_model(name: &str) -> Result<l6fw::Model, String> {
    match name {
        "l6" => Ok(l6fw::Model::L6),
        "l6max" => Ok(l6fw::Model::L6max),
        other => Err(format!("unknown model {other}")),
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if config.version != 1 {
            return Err("configuration requires version=1".into());
        }
        if let Some(model) = &config.model {
            parse_model(model)?;
        }
        for model in config.firmware.keys().chain(config.emulator.keys()) {
            parse_model(model)?;
        }
        Ok(config)
    }

    /// Load a file, with relative paths resolved against its directory.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut config = Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let resolve = |file: &mut PathBuf| {
            if file.is_relative() {
                *file = base.join(&*file);
            }
        };
        config.firmware.values_mut().for_each(resolve);
        for emulator in config.emulator.values_mut() {
            emulator.sd_card.iter_mut().for_each(resolve);
            emulator.input_patch.values_mut().for_each(resolve);
        }
        Ok(config)
    }

    pub fn selected_model(&self) -> Option<l6fw::Model> {
        self.model
            .as_deref()
            .and_then(|name| parse_model(name).ok())
    }

    pub fn firmware(&self, model: l6fw::Model) -> Option<&PathBuf> {
        self.firmware.get(model_name(model))
    }

    pub fn sd_card(&self, model: l6fw::Model) -> Option<&PathBuf> {
        self.emulator.get(model_name(model))?.sd_card.as_ref()
    }

    /// A model's files by one-based channel, without checking that they exist, so
    /// that the GUI can show and replace a file that moved.
    pub fn input_patch_paths(&self, model: l6fw::Model) -> Option<BTreeMap<u32, PathBuf>> {
        let emulator = self.emulator.get(model_name(model))?;
        Some(
            emulator
                .input_patch
                .iter()
                .filter_map(|(channel, file)| Some((channel.parse().ok()?, file.clone())))
                .collect(),
        )
    }

    /// A model's files by one-based channel, or `None` when it has none.
    pub fn input_patch(
        &self,
        model: l6fw::Model,
    ) -> Result<Option<BTreeMap<u32, PathBuf>>, String> {
        let name = model_name(model);
        let Some(emulator) = self
            .emulator
            .get(name)
            .filter(|e| !e.input_patch.is_empty())
        else {
            return Ok(None);
        };
        let channels = crate::controls::audio_input_channels(model);
        emulator
            .input_patch
            .iter()
            .map(|(channel, file)| {
                let number = channel
                    .parse()
                    .ok()
                    .filter(|number| (1..=channels).contains(number))
                    .ok_or_else(|| {
                        format!("[emulator.{name}.input_patch] has no channel {channel}")
                    })?;
                if !file.is_file() {
                    return Err(format!(
                        "[emulator.{name}.input_patch] {} is missing",
                        file.display()
                    ));
                }
                Ok((number, file.clone()))
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
}

/// Create an empty configuration if there is none.
pub fn ensure_file(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, "version = 1\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// Change a saved value, keeping the file's comments and order. Creates the file.
fn edit(path: &Path, change: impl FnOnce(&mut toml_edit::DocumentMut)) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "version = 1\n".into(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    change(&mut document);
    let partial = path.with_extension("toml.partial");
    fs::write(&partial, document.to_string()).map_err(|e| e.to_string())?;
    fs::rename(partial, path).map_err(|e| e.to_string())
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not valid text", path.display()))
}

/// The table `emulator.MODEL`, created implicitly so that only its subtables print.
fn emulator_table(
    document: &mut toml_edit::DocumentMut,
    model: l6fw::Model,
) -> &mut toml_edit::Table {
    let emulator = document["emulator"].or_insert(toml_edit::table());
    emulator.as_table_mut().unwrap().set_implicit(true);
    emulator[model_name(model)]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .unwrap()
}

pub fn save_firmware(path: &Path, model: l6fw::Model, package: &Path) -> Result<(), String> {
    let package = path_text(package)?.to_owned();
    edit(path, |document| {
        document["model"] = toml_edit::value(model_name(model));
        document["firmware"].or_insert(toml_edit::table())[model_name(model)] =
            toml_edit::value(package);
    })
}

pub fn save_sd_card(path: &Path, model: l6fw::Model, card: &Path) -> Result<(), String> {
    let card = path_text(card)?.to_owned();
    edit(path, |document| {
        emulator_table(document, model)["sd_card"] = toml_edit::value(card);
    })
}

pub fn save_input_patch(
    path: &Path,
    model: l6fw::Model,
    patch: &BTreeMap<u32, PathBuf>,
) -> Result<(), String> {
    let mut table = toml_edit::Table::new();
    for (channel, file) in patch {
        table[&channel.to_string()] = toml_edit::value(path_text(file)?);
    }
    edit(path, |document| {
        let emulator = emulator_table(document, model);
        if table.is_empty() {
            emulator.remove("input_patch");
        } else {
            emulator["input_patch"] = toml_edit::Item::Table(table);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_strict_and_a_patch_is_checked_only_when_used() {
        assert!(Config::parse("version=2").is_err());
        assert!(Config::parse("version=1\nother=1").is_err());
        assert!(Config::parse("version=1\nmodel='l7'").is_err());
        assert!(Config::parse("version=1\n[firmware]\nl7='x'").is_err());
        assert!(Config::parse("version=1\n[emulator.l7]\nsd_card='x'").is_err());
        assert!(Config::parse("version=1\n[test]\nqemu='q'").is_err());
        let config = Config::parse(
            "version=1\n[emulator.l6.input_patch]\n1='/missing.wav'\n[emulator.l6max.input_patch]\n9='/x.wav'",
        )
        .unwrap();
        assert!(
            config
                .input_patch(l6fw::Model::L6)
                .unwrap_err()
                .contains("missing")
        );
        assert!(
            config
                .input_patch(l6fw::Model::L6max)
                .unwrap_err()
                .contains("no channel 9")
        );
        assert!(
            Config::parse("version=1")
                .unwrap()
                .input_patch(l6fw::Model::L6)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn relative_paths_follow_the_file() {
        let directory = std::env::temp_dir().join(format!("l6-config-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join(FILE_NAME);
        fs::write(
            &file,
            "version=1\n[firmware]\nl6='L6.BIN'\n[emulator.l6]\nsd_card='c.img'\n",
        )
        .unwrap();
        let config = Config::load(&file).unwrap();
        assert_eq!(
            config.firmware(l6fw::Model::L6),
            Some(&directory.join("L6.BIN"))
        );
        assert_eq!(
            config.sd_card(l6fw::Model::L6),
            Some(&directory.join("c.img"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saving_keeps_comments_and_reads_back() {
        let directory = std::env::temp_dir().join(format!("l6-config-save-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join(FILE_NAME);
        save_firmware(&file, l6fw::Model::L6, Path::new("/a/L6.BIN")).unwrap();
        let text = format!("# mine\n{}", fs::read_to_string(&file).unwrap());
        fs::write(&file, text).unwrap();
        save_sd_card(&file, l6fw::Model::L6, Path::new("/a/card.img")).unwrap();
        let wav = directory.join("kick.wav");
        fs::write(&wav, b"").unwrap();
        save_input_patch(&file, l6fw::Model::L6, &BTreeMap::from([(2, wav.clone())])).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        let config = Config::load(&file).unwrap();
        assert_eq!(config.selected_model(), Some(l6fw::Model::L6));
        assert_eq!(
            config.firmware(l6fw::Model::L6),
            Some(&PathBuf::from("/a/L6.BIN"))
        );
        assert_eq!(
            config.sd_card(l6fw::Model::L6),
            Some(&PathBuf::from("/a/card.img"))
        );
        assert_eq!(
            config.input_patch(l6fw::Model::L6).unwrap().unwrap()[&2],
            wav
        );
        save_input_patch(&file, l6fw::Model::L6, &BTreeMap::new()).unwrap();
        assert!(
            Config::load(&file)
                .unwrap()
                .input_patch(l6fw::Model::L6)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
