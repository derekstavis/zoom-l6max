//! The web patcher hosts prebuilt SD patch modules. They are generated with
//! `tools/build-web-loadable-patches.sh`, which needs stock firmware, so CI
//! cannot rebuild them; it can check that the current loader accepts them.
use patch_loader_core::bundle::{ReadError, Reader};
use patch_loader_core::dynamic::{self, Hook};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Limits the resident loader passes to `read_header` (`patches/patch-loader`).
const MODULE_CAPACITY: usize = 0x60000;
const MAX_RELOCATIONS: usize = 256;

struct Slice<'a>(&'a [u8]);

impl Reader for Slice<'_> {
    type Error = ();
    fn read(&mut self, destination: &mut [u8]) -> Result<usize, Self::Error> {
        let count = destination.len().min(self.0.len());
        destination[..count].copy_from_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        Ok(count)
    }
    fn finish(&mut self) -> Result<(), ReadError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ReadError::Invalid)
        }
    }
}

#[test]
fn hosted_modules_load_with_the_current_loader() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/public/loadable-patches");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let modules = manifest["modules"].as_array().unwrap();
    assert!(!modules.is_empty());
    for module in modules {
        let file = module["file"].as_str().unwrap();
        let bytes = fs::read(root.join(file)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            module["sha256"].as_str().unwrap(),
            "{file}: manifest hash"
        );
        assert_eq!(
            module["format_version"].as_u64(),
            Some(dynamic::FORMAT_VERSION.into()),
            "{file}: manifest format version"
        );
        assert_eq!(
            module["loader_abi"].as_u64(),
            Some(dynamic::LOADER_ABI.into()),
            "{file}: manifest loader ABI"
        );

        let mut reader = Slice(&bytes);
        let pending = dynamic::read_header(
            &mut reader,
            MODULE_CAPACITY,
            dynamic::MAX_HOOKS_PER_MODULE,
            MAX_RELOCATIONS,
        )
        .unwrap_or_else(|error| panic!("{file}: loader rejects header: {error:?}"));
        let mut code = vec![0; pending.header.memory_len];
        let mut hooks = [Hook::default(); dynamic::MAX_HOOKS_PER_MODULE];
        dynamic::read_body(&mut reader, pending, &mut code, &mut hooks, 0, 0x2000_0000)
            .unwrap_or_else(|error| panic!("{file}: loader rejects body: {error:?}"));
    }

    // Every hosted module is listed, so none can be served unchecked.
    let mut hosted = Vec::new();
    for model in fs::read_dir(&root).unwrap().flatten() {
        if model.path().is_dir() {
            for file in fs::read_dir(model.path()).unwrap().flatten() {
                let name = file.file_name().into_string().unwrap();
                hosted.push(format!("{}/{name}", model.file_name().to_string_lossy()));
            }
        }
    }
    hosted.sort();
    let mut listed: Vec<String> = modules
        .iter()
        .map(|module| module["file"].as_str().unwrap().to_owned())
        .collect();
    listed.sort();
    assert_eq!(hosted, listed);
}
