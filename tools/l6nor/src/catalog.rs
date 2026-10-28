//! Revision-scoped settings-bank catalogs.
//!
//! A catalog describes one model and firmware revision's persisted settings:
//! bank slots, payload layout, field ranges and defaults. A catalog applies to
//! an installed MAIN image only when the image hash matches, or when every code
//! fingerprint matches. A fingerprint is the SHA-256 of a MAIN code range that
//! handles settings; patched images that leave those ranges untouched still
//! match.

use l6fw::Model;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const EMBEDDED: &[&str] = &[
    include_str!("../catalog/l6max-0110-nor-settings.json"),
    include_str!("../catalog/l6-0110-nor-settings.json"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    U8,
    I8,
    U16,
    I16,
    U32,
    Bytes,
}

impl Kind {
    pub fn width(self) -> usize {
        match self {
            Kind::U8 | Kind::I8 | Kind::Bytes => 1,
            Kind::U16 | Kind::I16 => 2,
            Kind::U32 => 4,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub offset: usize,
    pub kind: Kind,
    /// Element count; 1 for scalars.
    pub count: usize,
    pub stride: usize,
    /// Byte size of a `Bytes` field.
    pub size: usize,
    pub min: i64,
    pub max: i64,
    /// Per-element defaults, when the catalog states them.
    pub default: Option<Vec<i64>>,
}

#[derive(Clone, Debug)]
pub struct Fingerprint {
    pub name: String,
    pub address: u32,
    pub size: usize,
    pub sha256: String,
}

/// The NOR image holding a model's settings banks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// `main-nor.bin`, the MAIN MCU's 2 MiB FlexSPI NOR (L6max).
    MainNor,
    /// `resource-nor.bin`, the 4 MiB external serial NOR (L6).
    ResourceNor,
}

impl Device {
    pub fn size(self) -> usize {
        match self {
            Device::MainNor => crate::nor::MAIN_NOR_SIZE,
            Device::ResourceNor => crate::nor::RESOURCE_NOR_SIZE,
        }
    }

    pub fn file(self) -> &'static str {
        match self {
            Device::MainNor => "main-nor.bin",
            Device::ResourceNor => "resource-nor.bin",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BankLayout {
    pub device: Device,
    pub slots: [usize; 2],
    pub slot_size: usize,
    pub payload_offset: usize,
    pub payload_size: usize,
}

#[derive(Clone, Debug)]
pub struct MixerBlock {
    pub index: usize,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct MixerLayout {
    pub block_size: usize,
    pub blocks: Vec<MixerBlock>,
    pub channel_records: usize,
    pub record_size: usize,
    pub record_default: Vec<u8>,
    pub block_tail_defaults: Vec<(usize, Vec<u8>)>,
    pub record_fields: Vec<Field>,
    pub block_fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub model: Model,
    pub system_version: String,
    pub main_size: usize,
    pub main_sha256: String,
    pub fingerprints: Vec<Fingerprint>,
    pub bank: BankLayout,
    pub mixer: MixerLayout,
    pub fields: Vec<Field>,
    pub volatile_ranges: Vec<(usize, usize)>,
}

/// How an installed MAIN image was matched to a catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// The MAIN image is the catalog's exact reference image.
    Exact,
    /// The image differs, but every code fingerprint matches.
    Fingerprints,
}

impl Match {
    pub fn as_str(self) -> &'static str {
        match self {
            Match::Exact => "exact",
            Match::Fingerprints => "fingerprints",
        }
    }
}

/// Every embedded catalog, parsed and validated.
pub fn embedded() -> Vec<Catalog> {
    EMBEDDED
        .iter()
        .map(|text| Catalog::parse(text).expect("embedded settings catalog is valid"))
        .collect()
}

/// Find the catalog for an installed MAIN image of the given model.
pub fn for_main(model: Model, main: &[u8]) -> Result<(Catalog, Match), String> {
    let mut reasons = Vec::new();
    for catalog in embedded().into_iter().filter(|c| c.model == model) {
        match catalog.matches(main) {
            Ok(found) => return Ok((catalog, found)),
            Err(reason) => reasons.push(format!("{}: {reason}", catalog.system_version)),
        }
    }
    if reasons.is_empty() {
        Err(format!(
            "no settings catalog exists for {}",
            model_name(model)
        ))
    } else {
        Err(format!(
            "no {} settings catalog matches this MAIN image ({})",
            model_name(model),
            reasons.join("; ")
        ))
    }
}

pub fn model_name(model: Model) -> &'static str {
    match model {
        Model::L6 => "l6",
        Model::L6max => "l6max",
    }
}

pub fn main_base(model: Model) -> u32 {
    match model {
        Model::L6 => 0x80001000,
        Model::L6max => 0x80000000,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn number(text: &str) -> Result<usize, String> {
    match text.strip_prefix("0x") {
        Some(hex) => usize::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| format!("invalid catalog number {text:?}"))
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err(format!("odd-length hex string {text:?}"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    format: String,
    model: String,
    system_version: String,
    main_size: String,
    main_sha256: String,
    fingerprints: Vec<RawFingerprint>,
    bank: RawBank,
    mixer: RawMixer,
    fields: Vec<RawField>,
    volatile_ranges: Vec<RawRange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFingerprint {
    name: String,
    address: String,
    size: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBank {
    device: String,
    slots: [String; 2],
    slot_size: String,
    payload_offset: String,
    payload_size: String,
    markers: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    index: usize,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTail {
    offset: String,
    bytes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMixer {
    block_size: String,
    blocks: Vec<RawBlock>,
    channel_records: usize,
    record_size: String,
    record_default: String,
    block_tail_defaults: Vec<RawTail>,
    record_fields: Vec<RawField>,
    block_fields: Vec<RawField>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    name: String,
    offset: String,
    kind: String,
    count: Option<usize>,
    stride: Option<usize>,
    size: Option<String>,
    min: Option<i64>,
    max: Option<i64>,
    default: Option<Value>,
    default_hex: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    offset: String,
    size: String,
}

impl RawField {
    fn parse(self) -> Result<Field, String> {
        let kind = match self.kind.as_str() {
            "u8" => Kind::U8,
            "i8" => Kind::I8,
            "u16" => Kind::U16,
            "i16" => Kind::I16,
            "u32" => Kind::U32,
            "bytes" => Kind::Bytes,
            other => return Err(format!("field {} has unknown kind {other}", self.name)),
        };
        let count = self.count.unwrap_or(1);
        let stride = self.stride.unwrap_or(kind.width());
        let size = match (&self.size, kind) {
            (Some(size), Kind::Bytes) => number(size)?,
            (None, Kind::Bytes) => return Err(format!("bytes field {} needs a size", self.name)),
            (Some(_), _) => return Err(format!("only bytes fields have a size: {}", self.name)),
            (None, _) => kind.width(),
        };
        let (min, max) = match kind {
            Kind::Bytes => (0, 0),
            _ => (
                self.min.ok_or(format!("field {} needs min", self.name))?,
                self.max.ok_or(format!("field {} needs max", self.name))?,
            ),
        };
        let default = match (self.default, self.default_hex) {
            (Some(Value::Number(n)), None) => {
                Some(vec![n.as_i64().ok_or("non-integer default")?; count])
            }
            (Some(Value::Array(values)), None) => Some(
                values
                    .iter()
                    .map(|v| v.as_i64().ok_or("non-integer default"))
                    .collect::<Result<_, _>>()?,
            ),
            (None, Some(hex)) => Some(hex_bytes(&hex)?.into_iter().map(i64::from).collect()),
            (None, None) => None,
            _ => return Err(format!("field {} has an invalid default", self.name)),
        };
        if default.as_ref().is_some_and(|d| d.len() != count) {
            return Err(format!(
                "field {} default length differs from count",
                self.name
            ));
        }
        Ok(Field {
            name: self.name,
            offset: number(&self.offset)?,
            kind,
            count,
            stride,
            size,
            min,
            max,
            default,
        })
    }
}

impl Field {
    /// Byte length covered by all elements.
    pub fn span(&self) -> usize {
        self.stride * (self.count - 1) + self.size
    }
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawCatalog = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if raw.format != "l6nor-settings-v1" {
            return Err(format!("unsupported catalog format {}", raw.format));
        }
        if raw.bank.markers != [1, 2, 3] {
            return Err("only the 1-2-3 wrapping marker sequence is supported".into());
        }
        let model = match raw.model.as_str() {
            "l6" => Model::L6,
            "l6max" => Model::L6max,
            other => return Err(format!("unknown model {other}")),
        };
        let parse_fields = |fields: Vec<RawField>| -> Result<Vec<Field>, String> {
            fields.into_iter().map(RawField::parse).collect()
        };
        let catalog = Catalog {
            model,
            system_version: raw.system_version,
            main_size: number(&raw.main_size)?,
            main_sha256: raw.main_sha256,
            fingerprints: raw
                .fingerprints
                .into_iter()
                .map(|a| {
                    Ok(Fingerprint {
                        name: a.name,
                        address: u32::try_from(number(&a.address)?).map_err(|e| e.to_string())?,
                        size: number(&a.size)?,
                        sha256: a.sha256,
                    })
                })
                .collect::<Result<_, String>>()?,
            bank: BankLayout {
                device: match raw.bank.device.as_str() {
                    "main-nor" => Device::MainNor,
                    "resource-nor" => Device::ResourceNor,
                    other => return Err(format!("unknown bank device {other}")),
                },
                slots: [number(&raw.bank.slots[0])?, number(&raw.bank.slots[1])?],
                slot_size: number(&raw.bank.slot_size)?,
                payload_offset: number(&raw.bank.payload_offset)?,
                payload_size: number(&raw.bank.payload_size)?,
            },
            mixer: MixerLayout {
                block_size: number(&raw.mixer.block_size)?,
                blocks: raw
                    .mixer
                    .blocks
                    .into_iter()
                    .map(|b| MixerBlock {
                        index: b.index,
                        name: b.name,
                    })
                    .collect(),
                channel_records: raw.mixer.channel_records,
                record_size: number(&raw.mixer.record_size)?,
                record_default: hex_bytes(&raw.mixer.record_default)?,
                block_tail_defaults: raw
                    .mixer
                    .block_tail_defaults
                    .into_iter()
                    .map(|t| Ok((number(&t.offset)?, hex_bytes(&t.bytes)?)))
                    .collect::<Result<_, String>>()?,
                record_fields: parse_fields(raw.mixer.record_fields)?,
                block_fields: parse_fields(raw.mixer.block_fields)?,
            },
            fields: parse_fields(raw.fields)?,
            volatile_ranges: raw
                .volatile_ranges
                .into_iter()
                .map(|r| Ok((number(&r.offset)?, number(&r.size)?)))
                .collect::<Result<_, String>>()?,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), String> {
        let payload = self.bank.payload_size;
        let mixer = &self.mixer;
        let mixer_end = mixer.block_size * mixer.blocks.len();
        if self.bank.payload_offset + payload > self.bank.slot_size {
            return Err("payload exceeds its bank slot".into());
        }
        if self
            .bank
            .slots
            .iter()
            .any(|slot| slot + self.bank.slot_size > self.bank.device.size())
        {
            return Err("bank slots exceed their NOR device".into());
        }
        if mixer.record_default.len() != mixer.record_size {
            return Err("record default does not match record size".into());
        }
        if mixer.record_size * mixer.channel_records > mixer.block_size {
            return Err("channel records exceed the mixer block".into());
        }
        for (index, block) in mixer.blocks.iter().enumerate() {
            if block.index != index {
                return Err("mixer blocks must be listed in index order".into());
            }
        }
        for (offset, bytes) in &mixer.block_tail_defaults {
            if offset + bytes.len() > mixer.block_size {
                return Err("block default exceeds the mixer block".into());
            }
        }
        for field in &mixer.record_fields {
            if field.offset + field.span() > mixer.record_size {
                return Err(format!("record field {} exceeds its record", field.name));
            }
        }
        for field in &mixer.block_fields {
            if field.offset + field.span() > mixer.block_size {
                return Err(format!("block field {} exceeds its block", field.name));
            }
        }
        for field in &self.fields {
            if field.offset < mixer_end || field.offset + field.span() > payload {
                return Err(format!(
                    "global field {} is outside the global region",
                    field.name
                ));
            }
        }
        for (offset, size) in &self.volatile_ranges {
            if offset + size > payload {
                return Err("volatile range exceeds the payload".into());
            }
        }
        Ok(())
    }

    /// Check whether an installed MAIN image (starting at its load base,
    /// possibly followed by erased padding) is the catalog's reference image
    /// or matches all of its code fingerprints.
    pub fn matches(&self, main: &[u8]) -> Result<Match, String> {
        let base = main_base(self.model);
        if main
            .get(..self.main_size)
            .is_some_and(|image| sha256(image) == self.main_sha256)
        {
            return Ok(Match::Exact);
        }
        for fingerprint in &self.fingerprints {
            let start = fingerprint
                .address
                .checked_sub(base)
                .ok_or("fingerprint below MAIN base")? as usize;
            let bytes = main
                .get(start..start + fingerprint.size)
                .ok_or_else(|| format!("MAIN image does not contain {}", fingerprint.name))?;
            if sha256(bytes) != fingerprint.sha256 {
                return Err(format!("code fingerprint {} differs", fingerprint.name));
            }
        }
        Ok(Match::Fingerprints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalogs_parse() {
        let catalogs = embedded();
        assert_eq!(catalogs.len(), 2);
        for (catalog, model, mixer_end, cc_count, device) in [
            (&catalogs[0], Model::L6max, 0xe1c, 94, Device::MainNor),
            (&catalogs[1], Model::L6, 0xf1e, 66, Device::ResourceNor),
        ] {
            assert_eq!(catalog.model, model);
            assert_eq!(catalog.bank.device, device);
            assert_eq!(
                catalog.mixer.blocks.len() * catalog.mixer.block_size,
                mixer_end
            );
            let cc = catalog
                .fields
                .iter()
                .find(|f| f.name == "midi_cc_number")
                .unwrap();
            assert_eq!(cc.default.as_ref().unwrap().len(), cc_count);
        }
    }

    #[test]
    fn fingerprints_reject_a_different_image() {
        let catalog = &embedded()[0];
        let image = vec![0u8; 0x50000];
        assert!(catalog.matches(&image).unwrap_err().contains("differs"));
    }
}
