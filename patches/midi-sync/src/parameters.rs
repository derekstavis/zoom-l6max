#![cfg_attr(test, allow(dead_code))]
//! Mixer parameter identifiers and the sync request, independent of the firmware.
//!
//! The stock Control Change sender takes a parameter identifier, which the
//! user's CC map (editor message `31 0E`) translates into a controller number.
//! Identifiers form ten groups of eight physical channels, one group per blue
//! button setting (`SOURCES`/`GROUP_BASE`), plus four single global toggles
//! (`GLOBAL_TOGGLES`) that aren't per channel at all.

/// Blue-button settings in the order the encoder dispatcher numbers them.
pub const MODES: usize = 10;
/// Physical channels with a channel encoder.
pub const CHANNELS: usize = 8;
/// Controller 127 can never be assigned in the CC map (assignments above 119
/// are stored as unassigned), so it cannot collide with a mixer parameter.
pub const SYNC_REQUEST_CC: u8 = 127;
/// A controller's button-down value triggers; release (0) is ignored.
pub const SYNC_REQUEST_MIN_VALUE: u8 = 64;
/// Highest mixer channel index the stock setters accept.
pub const MAX_MIXER_CHANNEL: u32 = 11;

/// First parameter identifier of each setting's group of eight channels.
const GROUP_BASE: [u8; MODES] = [0x00, 0x08, 0x10, 0x18, 0x28, 0x30, 0x38, 0x20, 0x40, 0x48];

/// Which stock getter, and with which extra argument, reads a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    EqGain(u32),
    EqFrequency(u32),
    Aux(u32),
    Efx,
    SubMix,
    Pan,
    Level,
}

/// Getter for each setting; matches the order of the blue-button titles.
pub const SOURCES: [Source; MODES] = [
    Source::EqGain(0),
    Source::EqFrequency(1),
    Source::EqGain(1),
    Source::EqGain(2),
    Source::Aux(0),
    Source::Aux(1),
    Source::Efx,
    Source::SubMix,
    Source::Pan,
    Source::Level,
];

/// Identifier the stock sender expects for `mode` on physical `channel`.
pub fn parameter_id(mode: usize, channel: usize) -> Option<u8> {
    if mode >= MODES || channel >= CHANNELS {
        return None;
    }
    Some(GROUP_BASE[mode] + channel as u8)
}

/// True for a Control Change 127 on the configured channel with a pressed value.
pub fn is_sync_request(status: u8, controller: u8, value: u8, configured_channel: u8) -> bool {
    status & 0xf0 == 0xb0
        && status & 0x0f == configured_channel
        && controller == SYNC_REQUEST_CC
        && value >= SYNC_REQUEST_MIN_VALUE
}

/// A button-type parameter that is a single global toggle rather than one of
/// eight per-channel values — so, unlike `SOURCES`, each entry here is sent
/// exactly once per sync, not once per physical channel. `slot` is the fixed
/// index `MIXER_GET_CHANNEL_FLAG` (in `firmware.rs`) expects; `id` is the
/// parameter identifier the stock sender expects, matching `0x54 + <physical
/// button index>` the same way the per-channel groups match `GROUP_BASE +
/// <channel>`. Confirmed for these four; see `firmware.rs` for what's still open.
pub struct GlobalToggle {
    pub slot: u32,
    pub id: u8,
}

/// In physical button order (4, 5, 6, 7), which is also ascending identifier
/// order: CH 5 MONO x2, CH 6 MONO x2, USB 1/2, USB 3/4.
pub const GLOBAL_TOGGLES: [GlobalToggle; 4] = [
    GlobalToggle { slot: 4, id: 0x58 },
    GlobalToggle { slot: 6, id: 0x59 },
    GlobalToggle { slot: 8, id: 0x5A },
    GlobalToggle { slot: 10, id: 0x5B },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_unique_and_cover_eighty_parameters() {
        let mut seen = [false; 0x50];
        for mode in 0..MODES {
            for channel in 0..CHANNELS {
                let id = parameter_id(mode, channel).unwrap() as usize;
                assert!(id < 0x50 && !seen[id], "duplicate or out of range: {id:#x}");
                seen[id] = true;
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn identifiers_match_the_stock_dispatch_table() {
        let first = [0x00, 0x08, 0x10, 0x18, 0x28, 0x30, 0x38, 0x20, 0x40, 0x48];
        for (mode, id) in first.into_iter().enumerate() {
            assert_eq!(parameter_id(mode, 0), Some(id));
            assert_eq!(parameter_id(mode, 7), Some(id + 7));
        }
        assert_eq!(parameter_id(MODES, 0), None);
        assert_eq!(parameter_id(0, CHANNELS), None);
    }

    #[test]
    fn global_toggles_have_unique_identifiers_outside_the_per_channel_range() {
        let mut seen = [false; 0x5e];
        for toggle in GLOBAL_TOGGLES {
            let id = toggle.id as usize;
            assert!(id >= 0x50 && id < 0x5e, "out of the button-type range: {id:#x}");
            assert!(!seen[id], "duplicate identifier: {id:#x}");
            seen[id] = true;
            assert!(toggle.slot <= MAX_MIXER_CHANNEL, "slot out of range: {}", toggle.slot);
        }
    }

    #[test]
    fn sync_request_requires_channel_controller_and_pressed_value() {
        assert!(is_sync_request(0xb0, 127, 127, 0));
        assert!(is_sync_request(0xb5, 127, 64, 5));
        assert!(!is_sync_request(0xb5, 127, 127, 4), "other channel");
        assert!(!is_sync_request(0xb0, 126, 127, 0), "other controller");
        assert!(!is_sync_request(0xb0, 127, 63, 0), "released");
        assert!(!is_sync_request(0x90, 127, 127, 0), "not a control change");
        assert!(!is_sync_request(0xf2, 127, 127, 2), "system message");
    }
}
