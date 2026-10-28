//! A simulator device's NOR images: MAIN NOR and, for L6, the external
//! resource NOR that holds its settings banks.

use crate::{
    bank::{self, ActiveBank},
    catalog::{self, Catalog, Device as Storage, Match},
    nor::{self, MainNor},
    settings::Settings,
    state::{read_device_file, write_device_file},
};
use l6fw::Model;
use std::path::{Path, PathBuf};

pub const MAIN_NOR_FILE: &str = "main-nor.bin";
pub const RESOURCE_NOR_FILE: &str = "resource-nor.bin";

pub struct Device {
    pub main: MainNor,
    pub resource: Option<Vec<u8>>,
    /// Images created from firmware rather than read from the state.
    pub seeded_main: bool,
    pub seeded_resource: bool,
}

impl Device {
    /// Load a state directory's NOR images, seeding missing ones from
    /// `firmware_dir` exactly as QEMU's first persistent boot would.
    pub fn load(state_dir: &Path, firmware_dir: Option<&Path>) -> Result<Self, String> {
        let (main, seeded_main) =
            match read_device_file(&state_dir.join(MAIN_NOR_FILE))? {
                Some(bytes) => (MainNor::from_bytes(bytes)?, false),
                None => (
                    MainNor::seed(firmware_dir.ok_or(
                        "the state has no installed MAIN NOR; supply firmware to seed one",
                    )?)?,
                    true,
                ),
            };
        let mut device = Self {
            main,
            resource: None,
            seeded_main,
            seeded_resource: false,
        };
        if device.main.model()? == Model::L6 {
            match read_device_file(&state_dir.join(RESOURCE_NOR_FILE))? {
                Some(bytes) if bytes.len() == nor::RESOURCE_NOR_SIZE => {
                    device.resource = Some(bytes)
                }
                Some(_) => return Err("resource NOR must contain 4 MiB".into()),
                None => {
                    if let Some(firmware) = firmware_dir {
                        device.resource = Some(nor::seed_resource(firmware)?);
                        device.seeded_resource = true;
                    }
                }
            }
        }
        Ok(device)
    }

    /// Open a CLI path: a state directory, or a bare `main-nor.bin` (which
    /// cannot reach an L6 resource NOR).
    pub fn open(path: &Path) -> Result<(Self, PathBuf), String> {
        if path.is_dir() {
            if !path.join(MAIN_NOR_FILE).exists() {
                return Err(format!("{} has no {MAIN_NOR_FILE}", path.display()));
            }
            return Ok((Self::load(path, None)?, path.to_path_buf()));
        }
        let bytes = read_device_file(path)?
            .ok_or_else(|| format!("{} is missing or empty", path.display()))?;
        let device = Self {
            main: MainNor::from_bytes(bytes)?,
            resource: None,
            seeded_main: false,
            seeded_resource: false,
        };
        Ok((
            device,
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        ))
    }

    pub fn model(&self) -> Result<Model, String> {
        self.main.model()
    }

    pub fn catalog(&self) -> Result<(Catalog, Match), String> {
        catalog::for_main(self.model()?, self.main.main_region())
    }

    pub fn storage(&self, catalog: &Catalog) -> Result<&[u8], String> {
        match catalog.bank.device {
            Storage::MainNor => Ok(self.main.bytes()),
            Storage::ResourceNor => self.resource.as_deref().ok_or_else(|| {
                format!("settings live in {RESOURCE_NOR_FILE}; pass the state directory")
            }),
        }
    }

    fn storage_mut(&mut self, catalog: &Catalog) -> Result<&mut [u8], String> {
        match catalog.bank.device {
            Storage::MainNor => Ok(self.main.bytes_mut()),
            Storage::ResourceNor => self.resource.as_deref_mut().ok_or_else(|| {
                format!("settings live in {RESOURCE_NOR_FILE}; pass the state directory")
            }),
        }
    }

    pub fn active_bank(&self, catalog: &Catalog) -> Result<Option<ActiveBank>, String> {
        Ok(bank::active(self.storage(catalog)?, catalog))
    }

    /// The settings the firmware would load, or `None` when it would load
    /// its defaults.
    pub fn settings(&self, catalog: &Catalog) -> Result<Option<Settings>, String> {
        bank::payload(self.storage(catalog)?, catalog)
            .map(|payload| Settings::from_payload(catalog, payload))
            .transpose()
    }

    pub fn store_settings(&mut self, settings: &Settings) -> Result<ActiveBank, String> {
        let catalog = settings.catalog().clone();
        bank::store(self.storage_mut(&catalog)?, &catalog, settings.payload())
    }

    /// Write every image back to its file in `state_dir`.
    pub fn save(&self, state_dir: &Path) -> Result<(), String> {
        write_device_file(&state_dir.join(MAIN_NOR_FILE), self.main.bytes())?;
        if let Some(resource) = &self.resource {
            write_device_file(&state_dir.join(RESOURCE_NOR_FILE), resource)?;
        }
        Ok(())
    }
}
