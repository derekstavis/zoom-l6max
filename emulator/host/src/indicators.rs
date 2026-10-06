//! Firmware-derived wiring of the eight nine-segment encoder rings.
//! Main firmware 0x8000c4f0 indexes nine LEDs per channel; the physical
//! (common, serial column) pairs are at 0x800c460c. SPI bytes shift MSB first.
//! Panel 0x08002d88 reverses byte order and uses MSB-first bit indices:
//! firmware column n therefore occupies shift-register bit n ^ 7.
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
#[cfg(test)]
mod tests {
    use super::*;
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
pub fn leds(rows: [u32; 8]) -> [bool; 72] {
    LED_PINS.map(|(row, column)| rows[row] & (1 << (column ^ 7)) != 0)
}
