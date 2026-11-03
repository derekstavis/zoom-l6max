//! A settings-bank payload addressed through its catalog.
//!
//! Paths name catalog fields:
//!
//! - `battery_type`, `midi_cc_number[3]` for global fields;
//! - `mixer.current.efx_type`, `mixer.scene-0.efx_param[4]` for mixer-block fields;
//! - `mixer.current.channel[2].mute` for per-channel record fields.
//!
//! Mixer blocks may be named or numbered (`mixer.4.channel[0].pan`).

use crate::catalog::{Catalog, Field, Kind};
use std::fmt;

#[derive(Clone)]
pub struct Settings {
    catalog: Catalog,
    payload: Vec<u8>,
}

/// One addressed element: absolute payload offset plus the field describing it.
#[derive(Clone, Debug)]
pub struct Location<'a> {
    pub path: String,
    pub offset: usize,
    pub field: &'a Field,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Int(i64),
    Bytes(Vec<u8>),
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldValue::Int(value) => write!(f, "{value}"),
            FieldValue::Bytes(bytes) => {
                for byte in bytes {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    }
}

fn split_index(segment: &str) -> Result<(&str, Option<usize>), String> {
    match segment.split_once('[') {
        None => Ok((segment, None)),
        Some((name, rest)) => {
            let index = rest
                .strip_suffix(']')
                .and_then(|i| i.parse().ok())
                .ok_or_else(|| format!("invalid index in {segment:?}"))?;
            Ok((name, Some(index)))
        }
    }
}

fn element<'a>(
    fields: &'a [Field],
    segment: &str,
    base: usize,
    prefix: &str,
) -> Result<Location<'a>, String> {
    let (name, index) = split_index(segment)?;
    let field = fields
        .iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("unknown field {prefix}{name}"))?;
    let index = match (index, field.count) {
        (None, 1) => 0,
        (None, count) => return Err(format!("{prefix}{name} has {count} elements; add an index")),
        (Some(i), count) if i < count => i,
        (Some(i), count) => return Err(format!("{prefix}{name}[{i}] is outside 0..{count}")),
    };
    let path = if field.count == 1 {
        format!("{prefix}{name}")
    } else {
        format!("{prefix}{name}[{index}]")
    };
    Ok(Location {
        path,
        offset: base + field.offset + index * field.stride,
        field,
    })
}

fn read_int(bytes: &[u8], kind: Kind) -> i64 {
    match kind {
        Kind::U8 => i64::from(bytes[0]),
        Kind::I8 => i64::from(bytes[0] as i8),
        Kind::U16 => i64::from(u16::from_le_bytes([bytes[0], bytes[1]])),
        Kind::I16 => i64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        Kind::U32 => i64::from(u32::from_le_bytes(bytes[..4].try_into().unwrap())),
        Kind::Bytes => unreachable!("bytes fields are not integers"),
    }
}

fn write_int(bytes: &mut [u8], kind: Kind, value: i64) {
    match kind {
        Kind::U8 | Kind::I8 => bytes[0] = value as u8,
        Kind::U16 | Kind::I16 => bytes[..2].copy_from_slice(&(value as u16).to_le_bytes()),
        Kind::U32 => bytes[..4].copy_from_slice(&(value as u32).to_le_bytes()),
        Kind::Bytes => unreachable!("bytes fields are not integers"),
    }
}

impl Settings {
    /// The payload the firmware uses when no valid bank exists: six default mixer blocks followed by default globals; every
    /// other byte is zero.
    pub fn defaults(catalog: &Catalog) -> Self {
        let mut payload = vec![0; catalog.bank.payload_size];
        let mixer = &catalog.mixer;
        for block in 0..mixer.blocks.len() {
            let base = block * mixer.block_size;
            for record in 0..mixer.channel_records {
                let start = base + record * mixer.record_size;
                payload[start..start + mixer.record_size].copy_from_slice(&mixer.record_default);
            }
            for (offset, bytes) in &mixer.block_tail_defaults {
                payload[base + offset..base + offset + bytes.len()].copy_from_slice(bytes);
            }
        }
        for field in &catalog.fields {
            if let Some(defaults) = &field.default {
                for (index, value) in defaults.iter().enumerate() {
                    let offset = field.offset + index * field.stride;
                    write_int(&mut payload[offset..], field.kind, *value);
                }
            }
        }
        Self {
            catalog: catalog.clone(),
            payload,
        }
    }

    pub fn from_payload(catalog: &Catalog, payload: &[u8]) -> Result<Self, String> {
        if payload.len() != catalog.bank.payload_size {
            return Err("settings payload has the wrong size".into());
        }
        Ok(Self {
            catalog: catalog.clone(),
            payload: payload.to_vec(),
        })
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn block_index(&self, name: &str) -> Result<usize, String> {
        let blocks = &self.catalog.mixer.blocks;
        blocks
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.index)
            .or_else(|| name.parse().ok().filter(|i| *i < blocks.len()))
            .ok_or_else(|| {
                let names: Vec<_> = blocks.iter().map(|b| b.name.as_str()).collect();
                format!(
                    "unknown mixer block {name:?}; use one of {}",
                    names.join(", ")
                )
            })
    }

    pub fn locate(&self, path: &str) -> Result<Location<'_>, String> {
        let catalog = &self.catalog;
        let Some(rest) = path.strip_prefix("mixer.") else {
            return element(&catalog.fields, path, 0, "");
        };
        let (block, rest) = rest
            .split_once('.')
            .ok_or_else(|| format!("{path}: expected mixer.<block>.<field>"))?;
        let block_name = &catalog.mixer.blocks[self.block_index(block)?].name;
        let base = self.block_index(block)? * catalog.mixer.block_size;
        match rest.split_once('.') {
            None => element(
                &catalog.mixer.block_fields,
                rest,
                base,
                &format!("mixer.{block_name}."),
            ),
            Some((channel, field)) => {
                let (name, index) = split_index(channel)?;
                let index = index
                    .filter(|_| name == "channel")
                    .ok_or_else(|| format!("{path}: expected mixer.<block>.channel[N].<field>"))?;
                if index >= catalog.mixer.channel_records {
                    return Err(format!(
                        "{path}: channel {index} is outside 0..{}",
                        catalog.mixer.channel_records
                    ));
                }
                element(
                    &catalog.mixer.record_fields,
                    field,
                    base + index * catalog.mixer.record_size,
                    &format!("mixer.{block_name}.channel[{index}]."),
                )
            }
        }
    }

    pub fn get(&self, path: &str) -> Result<FieldValue, String> {
        let location = self.locate(path)?;
        let bytes = &self.payload[location.offset..location.offset + location.field.size];
        Ok(match location.field.kind {
            Kind::Bytes => FieldValue::Bytes(bytes.to_vec()),
            kind => FieldValue::Int(read_int(bytes, kind)),
        })
    }

    pub fn set(&mut self, path: &str, value: &FieldValue) -> Result<(), String> {
        let location = self.locate(path)?;
        let (offset, field) = (location.offset, location.field.clone());
        match (field.kind, value) {
            (Kind::Bytes, FieldValue::Bytes(bytes)) => {
                if bytes.len() > field.size {
                    return Err(format!("{path} holds at most {} bytes", field.size));
                }
                let target = &mut self.payload[offset..offset + field.size];
                target.fill(0);
                target[..bytes.len()].copy_from_slice(bytes);
            }
            (Kind::Bytes, FieldValue::Int(_)) => {
                return Err(format!("{path} is a byte field; give hex bytes"));
            }
            (_, FieldValue::Bytes(_)) => return Err(format!("{path} is an integer field")),
            (kind, FieldValue::Int(value)) => {
                if !(field.min..=field.max).contains(value) {
                    return Err(format!(
                        "{path} = {value} is outside the firmware range {}..={}",
                        field.min, field.max
                    ));
                }
                write_int(&mut self.payload[offset..], kind, *value);
            }
        }
        Ok(())
    }

    /// Every catalog element, in payload order, with its current value.
    pub fn entries(&self) -> Vec<(String, usize, FieldValue)> {
        let catalog = &self.catalog;
        let mut paths = Vec::new();
        let expand = |prefix: String, field: &Field, paths: &mut Vec<String>| {
            if field.count == 1 {
                paths.push(format!("{prefix}{}", field.name));
            } else {
                for i in 0..field.count {
                    paths.push(format!("{prefix}{}[{i}]", field.name));
                }
            }
        };
        for block in &catalog.mixer.blocks {
            for channel in 0..catalog.mixer.channel_records {
                for field in &catalog.mixer.record_fields {
                    expand(
                        format!("mixer.{}.channel[{channel}].", block.name),
                        field,
                        &mut paths,
                    );
                }
            }
            for field in &catalog.mixer.block_fields {
                expand(format!("mixer.{}.", block.name), field, &mut paths);
            }
        }
        for field in &catalog.fields {
            expand(String::new(), field, &mut paths);
        }
        let mut entries: Vec<_> = paths
            .into_iter()
            .map(|path| {
                let offset = self.locate(&path).unwrap().offset;
                let value = self.get(&path).unwrap();
                (path, offset, value)
            })
            .collect();
        entries.sort_by_key(|(_, offset, _)| *offset);
        entries
    }

    /// Name the catalog element covering a payload offset, when one does.
    pub fn describe(&self, offset: usize) -> Option<String> {
        self.entries()
            .into_iter()
            .find(|(path, start, _)| {
                let size = self.locate(path).unwrap().field.size;
                (*start..*start + size).contains(&offset)
            })
            .map(|(path, _, _)| path)
    }
}

/// Parse a command-line or TOML scalar: decimal, `0x` hex, or `hex:` bytes.
pub fn parse_value(text: &str) -> Result<FieldValue, String> {
    if let Some(hex) = text.strip_prefix("hex:") {
        if hex.len() % 2 != 0 {
            return Err(format!("odd-length hex value {text:?}"));
        }
        return (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()
            .map(FieldValue::Bytes);
    }
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed
        .map(FieldValue::Int)
        .map_err(|_| format!("invalid value {text:?}; use an integer, 0x hex or hex:bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use sha2::{Digest, Sha256};

    fn catalog() -> Catalog {
        catalog::embedded().remove(0)
    }

    #[test]
    fn defaults_match_the_firmware_defaults() {
        // SHA-256 of the L6max 1.10 default payload. Emulator analysis found it
        // identical to banks committed by the emulated firmware outside the
        // fields the session had changed.
        let settings = Settings::defaults(&catalog());
        assert_eq!(
            format!("{:x}", Sha256::digest(settings.payload())),
            "752e5790d279cb9e9b4065b020d8f4d0822a788287346ae01c2a7c2722b51972"
        );
        assert_eq!(settings.get("setup_required").unwrap(), FieldValue::Int(1));
        assert_eq!(
            settings.get("sound_pad_midi_note[3]").unwrap(),
            FieldValue::Int(0x41)
        );
        assert_eq!(
            settings.get("mixer.current.channel[0].pan").unwrap(),
            FieldValue::Int(0x40)
        );
        assert_eq!(
            settings.get("mixer.current.efx_param[6]").unwrap(),
            FieldValue::Int(504)
        );
        assert_eq!(
            settings.get("last_date_year").unwrap(),
            FieldValue::Int(2025)
        );
    }

    #[test]
    fn l6_defaults_match_the_firmware_defaulted_ram_image() {
        // SHA-256 of the L6 1.10 default payload. Emulator analysis found it identical
        // to the 0x17c4-byte image the emulated firmware built at 0x804439a0
        // when it loaded defaults.
        let l6 = catalog::embedded().remove(1);
        let mut settings = Settings::defaults(&l6);
        assert_eq!(
            format!("{:x}", Sha256::digest(settings.payload())),
            "a9fc79ac40e937f6a2afbdd48ed435359ba1ddbc13fb7ba48a1091e4f0f42204"
        );
        settings.set("saved_scenes", &FieldValue::Int(5)).unwrap();
        settings
            .set("mixer.scene-2.channel[9].unclassified", &FieldValue::Int(1))
            .unwrap_err();
        assert!(settings.set("saved_scenes", &FieldValue::Int(8)).is_err());
        assert_eq!(settings.payload()[0xf20], 5);
    }

    #[test]
    fn paths_address_globals_blocks_and_channels() {
        let mut settings = Settings::defaults(&catalog());
        settings.set("battery_type", &FieldValue::Int(2)).unwrap();
        settings
            .set("mixer.4.channel[11].mute", &FieldValue::Int(1))
            .unwrap();
        settings
            .set("mixer.scene-1.efx_type", &FieldValue::Int(3))
            .unwrap();
        assert_eq!(settings.payload()[0xe28], 2);
        assert_eq!(settings.payload()[4 * 0x25a + 11 * 0x18], 1);
        assert_eq!(settings.payload()[0x25a + 0x12e], 3);
        assert_eq!(settings.describe(0xe29).as_deref(), Some("battery_type"));
        assert_eq!(
            settings
                .locate("mixer.current.channel[11].mute")
                .unwrap()
                .path,
            "mixer.current.channel[11].mute"
        );
    }

    #[test]
    fn l6max_effect_return_fields_are_independent_and_bounded() {
        let mut settings = Settings::defaults(&catalog());
        let submix = "mixer.current.efx_return_submix";
        let mix_minus = "mixer.current.efx_return_mix_minus";
        assert_eq!(settings.get(submix).unwrap(), FieldValue::Int(0));
        assert_eq!(settings.get(mix_minus).unwrap(), FieldValue::Int(0));
        settings.set(submix, &FieldValue::Int(1)).unwrap();
        assert_eq!(settings.get(mix_minus).unwrap(), FieldValue::Int(0));
        settings.set(mix_minus, &FieldValue::Int(1)).unwrap();
        assert_eq!(
            &settings.payload()[4 * 0x25a + 0x159..4 * 0x25a + 0x15b],
            &[1, 1]
        );
        assert!(settings.set(submix, &FieldValue::Int(2)).is_err());
        assert!(settings.set(mix_minus, &FieldValue::Int(-1)).is_err());
        assert_eq!(
            settings.get("mixer.scene-0.efx_return_submix").unwrap(),
            FieldValue::Int(0)
        );
    }

    #[test]
    fn out_of_range_and_unknown_paths_are_rejected() {
        let mut settings = Settings::defaults(&catalog());
        assert!(settings.set("battery_type", &FieldValue::Int(3)).is_err());
        assert!(settings.set("midi_cc_number", &FieldValue::Int(1)).is_err());
        assert!(
            settings
                .set("midi_cc_number[94]", &FieldValue::Int(1))
                .is_err()
        );
        assert!(
            settings
                .set("mixer.current.channel[12].mute", &FieldValue::Int(1))
                .is_err()
        );
        assert!(
            settings
                .set("mixer.scene-9.efx_type", &FieldValue::Int(1))
                .is_err()
        );
        assert!(settings.set("no_such_field", &FieldValue::Int(1)).is_err());
    }

    #[test]
    fn values_parse() {
        assert_eq!(parse_value("12").unwrap(), FieldValue::Int(12));
        assert_eq!(parse_value("0x7e9").unwrap(), FieldValue::Int(2025));
        assert_eq!(
            parse_value("hex:3200").unwrap(),
            FieldValue::Bytes(vec![0x32, 0])
        );
        assert!(parse_value("x").is_err());
    }
}
