//! USB descriptor decoding for `usb-descriptors` observations.
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn device(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 18 || bytes[1] != 1 {
        return Err("malformed USB device descriptor".into());
    }
    Ok(json!({
        "usb": format!("{:x}.{:02x}", bytes[3], bytes[2]),
        "class": bytes[4],
        "subclass": bytes[5],
        "protocol": bytes[6],
        "ep0_max_packet": bytes[7],
        "vid": format!("{:04x}", word(bytes, 8)),
        "pid": format!("{:04x}", word(bytes, 10)),
        "release": format!("{:x}.{:02x}", bytes[13], bytes[12]),
        "manufacturer_string": bytes[14],
        "product_string": bytes[15],
        "serial_string": bytes[16],
        "configurations": bytes[17],
    }))
}

fn transfer(attributes: u8) -> &'static str {
    ["control", "isochronous", "bulk", "interrupt"][(attributes & 3) as usize]
}

/// Every descriptor in a configuration, in wire order, plus the endpoint
/// numbers used in each direction across all alternate settings.
pub fn configuration(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 9 || bytes[1] != 2 || word(bytes, 2) as usize != bytes.len() {
        return Err("malformed USB configuration descriptor".into());
    }
    let mut descriptors = Vec::new();
    let mut strings = BTreeSet::new();
    let (mut ins, mut outs) = (BTreeSet::new(), BTreeSet::new());
    let mut offset = 0;
    while offset < bytes.len() {
        let length = bytes[offset] as usize;
        if length < 2 || offset + length > bytes.len() {
            return Err(format!("malformed descriptor at offset {offset}"));
        }
        let d = &bytes[offset..offset + length];
        let decoded = match d[1] {
            2 if length >= 9 => {
                strings.insert(d[6]);
                json!({"type":"configuration","total_length":word(d, 2),"interfaces":d[4],"value":d[5],"string":d[6],"attributes":format!("{:02x}", d[7]),"max_power_ma":d[8] as u32 * 2})
            }
            4 if length >= 9 => {
                strings.insert(d[8]);
                json!({"type":"interface","number":d[2],"alternate":d[3],"endpoints":d[4],"class":d[5],"subclass":d[6],"protocol":d[7],"string":d[8]})
            }
            5 if length >= 7 => {
                let address = d[2];
                if address & 0x80 != 0 {
                    ins.insert(address & 15);
                } else {
                    outs.insert(address & 15);
                }
                let packet = word(d, 4);
                json!({"type":"endpoint","address":format!("{address:02x}"),"number":address & 15,"direction":if address & 0x80 != 0 { "in" } else { "out" },"transfer":transfer(d[3]),"attributes":format!("{:02x}", d[3]),"max_packet":packet & 0x7ff,"extra_transactions":(packet >> 11) & 3,"interval":d[6],"extra":hex(&d[7..])})
            }
            11 if length >= 8 => {
                strings.insert(d[7]);
                json!({"type":"interface-association","first_interface":d[2],"interfaces":d[3],"class":d[4],"subclass":d[5],"protocol":d[6],"string":d[7]})
            }
            0x24 => json!({"type":"class-interface","subtype":d.get(2),"raw":hex(d)}),
            0x25 => json!({"type":"class-endpoint","subtype":d.get(2),"raw":hex(d)}),
            kind => json!({"type":format!("{kind:#04x}"),"raw":hex(d)}),
        };
        descriptors.push(decoded);
        offset += length;
    }
    strings.remove(&0);
    Ok(json!({
        "descriptors": descriptors,
        "endpoints_in": ins,
        "endpoints_out": outs,
        "strings": strings,
    }))
}

/// A UTF-16LE string descriptor's text.
pub fn string(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 2
        || bytes[1] != 3
        || bytes[0] as usize > bytes.len()
        || !bytes[0].is_multiple_of(2)
    {
        return Err("malformed USB string descriptor".into());
    }
    let units: Vec<_> = bytes[2..bytes[0] as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_interfaces_endpoints_and_strings() {
        let mut config = vec![9, 2, 0, 0, 1, 1, 0, 0x80, 50];
        config.extend([9, 4, 0, 0, 2, 1, 3, 0, 4]);
        config.extend([7, 0x25, 1, 0, 0, 0, 0]);
        config.extend([9, 5, 0x81, 5, 0x40, 0x0a, 1, 0, 0]);
        config.extend([7, 5, 0x02, 2, 0, 2, 0]);
        config[2] = config.len() as u8;
        let decoded = super::configuration(&config).unwrap();
        assert_eq!(decoded["endpoints_in"], serde_json::json!([1]));
        assert_eq!(decoded["endpoints_out"], serde_json::json!([2]));
        assert_eq!(decoded["strings"], serde_json::json!([4]));
        let iso = &decoded["descriptors"][3];
        assert_eq!(iso["transfer"], "isochronous");
        assert_eq!(iso["max_packet"], 0x240);
        assert_eq!(iso["extra_transactions"], 1);
        assert_eq!(decoded["descriptors"][2]["type"], "class-endpoint");
        config[2] += 1;
        assert!(super::configuration(&config).is_err());
    }

    #[test]
    fn decodes_utf16_strings() {
        assert_eq!(super::string(&[6, 3, b'L', 0, b'6', 0]).unwrap(), "L6");
        assert!(super::string(&[5, 3, b'L', 0, b'6']).is_err());
    }
}
