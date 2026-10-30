//! Firmware-derived wiring of the eight nine-segment encoder rings.
//! Main firmware 0x8000c4f0 indexes nine LEDs per channel; the physical
//! (common, serial column) pairs are at 0x800c460c. SPI bytes shift MSB first.
//! Panel 0x08002d88 reverses byte order and uses MSB-first bit indices:
//! firmware column n therefore occupies shift-register bit n ^ 7.
//! L6 QEMU traces for CC 83–88 and isolated encoder turns verify that the first
//! six entries decode to their corresponding L6 ring outputs as well.
pub const RING_PINS: [[(usize, u32); 9]; 8] = [
    [
        (0, 2),
        (0, 3),
        (1, 2),
        (1, 3),
        (2, 2),
        (2, 3),
        (3, 2),
        (3, 3),
        (4, 2),
    ],
    [
        (4, 3),
        (5, 2),
        (5, 3),
        (6, 2),
        (6, 3),
        (7, 2),
        (7, 3),
        (4, 5),
        (5, 4),
    ],
    [
        (0, 4),
        (0, 5),
        (1, 4),
        (1, 5),
        (2, 4),
        (2, 5),
        (3, 4),
        (3, 5),
        (4, 4),
    ],
    [
        (5, 5),
        (6, 4),
        (6, 5),
        (7, 4),
        (7, 5),
        (0, 6),
        (0, 7),
        (1, 6),
        (1, 7),
    ],
    [
        (2, 6),
        (2, 7),
        (3, 6),
        (3, 7),
        (4, 6),
        (4, 7),
        (5, 6),
        (5, 7),
        (6, 6),
    ],
    [
        (6, 7),
        (7, 6),
        (7, 7),
        (0, 8),
        (0, 9),
        (1, 8),
        (1, 9),
        (2, 8),
        (2, 9),
    ],
    [
        (3, 8),
        (3, 9),
        (4, 8),
        (4, 9),
        (5, 8),
        (5, 9),
        (0, 10),
        (0, 11),
        (1, 10),
    ],
    [
        (1, 11),
        (2, 10),
        (2, 11),
        (3, 10),
        (3, 11),
        (4, 10),
        (4, 11),
        (5, 10),
        (5, 11),
    ],
];
pub fn rings(rows: [u32; 8]) -> [u32; 8] {
    RING_PINS.map(|pins| {
        pins.iter()
            .enumerate()
            .fold(0, |mask, (segment, &(row, column))| {
                mask | (((rows[row] >> (column ^ 7)) & 1) << segment)
            })
    })
}

/// PWM brightness for the nine encoder-ring segments on each channel.
pub fn ring_levels(levels: [[u8; 24]; 8]) -> [[u8; 9]; 8] {
    RING_PINS.map(|pins| pins.map(|(row, column)| levels[row][(column ^ 7) as usize]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn l6_master_meter_reads_each_side_independently() {
        let mut levels = [[0; 24]; 8];
        assert_eq!(l6_master_meter_levels(levels), [0, 0]);
        for &(row, bit) in &L6_MASTER_METER_PINS[0][..3] {
            levels[row][bit] = 45;
        }
        for &(row, bit) in &L6_MASTER_METER_PINS[1][..5] {
            levels[row][bit] = 45;
        }
        levels[0][10] = 3; // Partial scan while the sixth segment is off.
        assert_eq!(l6_master_meter_levels(levels), [3, 5]);
        levels[0][10] = 45;
        assert_eq!(l6_master_meter_levels(levels), [3, 6]);
    }
    #[test]
    fn every_output_lights_exactly_one_segment() {
        for (channel, pins) in RING_PINS.iter().enumerate() {
            for (segment, &(row, column)) in pins.iter().enumerate() {
                let mut rows = [0; 8];
                rows[row] = 1 << (column ^ 7);
                let mut expected = [0; 8];
                expected[channel] = 1 << segment;
                assert_eq!(rings(rows), expected);
            }
        }
        assert_eq!(rings([0xff0fc0; 8]), [0; 8]);
    }
}

/// Non-ring LED wiring at main firmware 0x800c457c (common, serial column).
pub const LED_PINS: [(usize, u32); 72] = [
    (0, 12),
    (0, 13),
    (1, 12),
    (1, 13),
    (2, 12),
    (2, 13),
    (3, 12),
    (3, 13),
    (5, 12),
    (5, 13),
    (2, 22),
    (4, 12),
    (4, 13),
    (5, 14),
    (5, 15),
    (6, 14),
    (6, 15),
    (0, 14),
    (0, 15),
    (1, 14),
    (1, 15),
    (2, 14),
    (2, 15),
    (3, 14),
    (3, 15),
    (4, 14),
    (4, 15),
    (1, 18),
    (1, 19),
    (2, 18),
    (2, 19),
    (1, 23),
    (5, 18),
    (1, 22),
    (0, 20),
    (0, 21),
    (1, 20),
    (1, 21),
    (2, 20),
    (2, 21),
    (3, 18),
    (3, 19),
    (4, 18),
    (4, 19),
    (0, 1),
    (1, 1),
    (2, 1),
    (3, 1),
    (4, 1),
    (5, 1),
    (6, 1),
    (7, 1),
    (0, 0),
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (0, 16),
    (1, 16),
    (2, 16),
    (3, 16),
    (0, 22),
    (0, 18),
    (0, 17),
    (1, 17),
    (2, 17),
    (3, 17),
    (0, 23),
    (0, 19),
];
/// Decode the two six-segment L6max MASTER meters from panel PWM duty.
pub fn l6max_master_meter_levels(levels: [[u8; 24]; 8]) -> [u8; 2] {
    [60, 66].map(|start| {
        LED_PINS[start..start + 6]
            .iter()
            .filter(|&&(row, column)| levels[row][(column ^ 7) as usize] != 0)
            .count() as u8
    })
}

pub fn leds(rows: [u32; 8]) -> [bool; 72] {
    LED_PINS.map(|(row, column)| rows[row] & (1 << (column ^ 7)) != 0)
}

/// L6 MASTER meter segments in bottom-to-top order, as observed in the
/// panel's 16-bit shift register. Unlike the L6max matrix, these columns
/// occupy the upper byte of the sampled latch.
const L6_MASTER_METER_PINS: [[(usize, usize); 6]; 2] = [
    [(3, 13), (4, 13), (5, 13), (6, 13), (7, 13), (0, 11)],
    [(3, 12), (4, 12), (5, 12), (6, 12), (7, 12), (0, 10)],
];

/// Decode the two six-segment MASTER meters from sampled panel PWM duty.
/// A lit segment measures about 45/255 with the L6 scan; the threshold
/// suppresses brief partial samples at output transitions.
pub fn l6_master_meter_levels(levels: [[u8; 24]; 8]) -> [u8; 2] {
    L6_MASTER_METER_PINS.map(|pins| {
        pins.iter()
            .filter(|&&(row, bit)| levels[row][bit] >= 16)
            .count() as u8
    })
}

/// L6 mute, mono, USB, phantom-power, and compressor lamps measured from
/// sequential panel events and stock USB Mixer Control CCs. These positions
/// are model-specific; the L6max LED table does not cover the six mute lamps.
const L6_BUTTON_PINS: [(usize, u32); 12] = [
    (4, 1 << 11),
    (4, 1 << 10),
    (5, 1 << 11),
    (5, 1 << 10),
    (6, 1 << 11),
    (6, 1 << 10),
    (5, 1 << 9),
    (5, 1 << 8),
    (6, 1 << 9),
    (6, 1 << 8),
    (7, 1 << 10),
    (7, 1 << 11),
];

pub fn l6_button_leds(rows: [u32; 8]) -> [bool; 12] {
    L6_BUTTON_PINS.map(|(row, mask)| rows[row] & mask != 0)
}

/// L6 SOUND PAD playback lamps in physical pad order, mapped from the
/// firmware's playback blink task and repeated QEMU indicator-row traces.
const L6_SOUND_PAD_PINS: [(usize, u32); 4] =
    [(3, 1 << 15), (3, 1 << 14), (4, 1 << 15), (4, 1 << 14)];

pub fn l6_sound_pad_leds(rows: [u32; 8]) -> [bool; 4] {
    L6_SOUND_PAD_PINS.map(|(row, mask)| rows[row] & mask != 0)
}

/// Five L6 effect-selection lamps in the order printed on the faceplate.
/// Successive EFX SEL events cycle through these independent outputs.
const L6_EFFECT_PINS: [(usize, u32); 5] = [
    (0, 1 << 12), // Hall (CC 00)
    (1, 1 << 13), // Room (CC 1A)
    (1, 1 << 12), // Spring (CC 33)
    (2, 1 << 13), // Delay (CC 4D)
    (2, 1 << 12), // Echo (CC 66)
];

pub fn l6_effect_leds(rows: [u32; 8]) -> [bool; 5] {
    L6_EFFECT_PINS.map(|(row, mask)| rows[row] & mask != 0)
}

/// L6 channel-strip modes, in faceplate order. These pins were observed
/// changing with their corresponding firmware selector events.
const L6_MODE_PINS: [(usize, u32); 9] = [
    (0, 1 << 9), // HIGH
    (0, 1 << 8), // FREQ
    (1, 1 << 9), // MID
    (1, 1 << 8), // LOW
    (2, 1 << 9), // AUX1
    (2, 1 << 8), // AUX2
    (3, 1 << 9), // EFX
    (3, 1 << 8), // PAN (L6 repurposes the L6max SUB-MIX lamp pin)
    (4, 1 << 8), // LEVEL
];

pub fn l6_mode_leds(rows: [u32; 8]) -> [bool; 9] {
    L6_MODE_PINS.map(|(row, mask)| rows[row] & mask != 0)
}

/// L6 scene lamps in faceplate order, measured while saving A, B, and C.
const L6_SCENE_PINS: [(usize, u32); 3] = [(2, 1 << 11), (2, 1 << 10), (3, 1 << 11)];

pub fn l6_scene_leds(rows: [u32; 8]) -> [bool; 3] {
    L6_SCENE_PINS.map(|(row, mask)| rows[row] & mask != 0)
}

/// L6 Record button lamp, observed after its MAIN event 31 press.
pub fn l6_record_led(rows: [u32; 8]) -> bool {
    rows[1] & (1 << 10) != 0
}

#[cfg(test)]
mod l6_tests {
    use super::*;

    #[test]
    fn l6_mute_outputs_follow_the_six_measured_matrix_bits() {
        for (channel, &(row, mask)) in L6_BUTTON_PINS[..6].iter().enumerate() {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 6];
            expected[channel] = true;
            assert_eq!(&l6_button_leds(rows)[..6], expected);
        }
        for (index, &(row, mask)) in L6_BUTTON_PINS.iter().enumerate().skip(6) {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 12];
            expected[index] = true;
            assert_eq!(l6_button_leds(rows), expected);
        }
        assert_eq!(l6_button_leds([0; 8]), [false; 12]);
    }

    #[test]
    fn l6_sound_pad_lamps_follow_the_four_playback_blink_bits() {
        for (pad, &(row, mask)) in L6_SOUND_PAD_PINS.iter().enumerate() {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 4];
            expected[pad] = true;
            assert_eq!(l6_sound_pad_leds(rows), expected);
        }
        assert_eq!(l6_sound_pad_leds([0; 8]), [false; 4]);
    }

    #[test]
    fn l6_effect_lamps_follow_the_five_sel_cycle_outputs() {
        for (effect, &(row, mask)) in L6_EFFECT_PINS.iter().enumerate() {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 5];
            expected[effect] = true;
            assert_eq!(l6_effect_leds(rows), expected);
        }
        assert_eq!(l6_effect_leds([0; 8]), [false; 5]);
    }

    #[test]
    fn l6_mode_lamps_follow_the_nine_selector_outputs() {
        for (mode, &(row, mask)) in L6_MODE_PINS.iter().enumerate() {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 9];
            expected[mode] = true;
            assert_eq!(l6_mode_leds(rows), expected);
        }
        assert_eq!(l6_mode_leds([0; 8]), [false; 9]);
    }

    #[test]
    fn l6_scene_lamps_follow_the_three_measured_outputs() {
        for (scene, &(row, mask)) in L6_SCENE_PINS.iter().enumerate() {
            let mut rows = [0; 8];
            rows[row] = mask;
            let mut expected = [false; 3];
            expected[scene] = true;
            assert_eq!(l6_scene_leds(rows), expected);
        }
        assert_eq!(l6_scene_leds([0; 8]), [false; 3]);
    }

    #[test]
    fn l6_record_lamp_uses_its_measured_matrix_bit() {
        let mut rows = [0; 8];
        rows[1] = 1 << 10;
        assert!(l6_record_led(rows));
        rows[1] = 0;
        assert!(!l6_record_led(rows));
    }
}
