//! Display units: physical EQ tables, normalized control range, and pan position.
struct Text {
    out: [u16; 24],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            out: [0; 24],
            len: 0,
        }
    }
    fn push(&mut self, value: u8) {
        if self.len < 23 {
            self.out[self.len] = value as u16;
            self.len += 1;
        }
    }
    fn text(&mut self, value: &[u8]) {
        for &c in value {
            self.push(c);
        }
    }
    fn number(&mut self, mut value: u32) {
        let mut digits = [0; 10];
        let mut len = 0;
        loop {
            digits[len] = b'0' + (value % 10) as u8;
            len += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for i in (0..len).rev() {
            self.push(digits[i]);
        }
    }
    fn decimal(&mut self, hundredths: u32) {
        self.number(hundredths / 100);
        self.push(b'.');
        self.push(b'0' + ((hundredths / 10) % 10) as u8);
        self.push(b'0' + (hundredths % 10) as u8);
    }
}
/// `gain` is the original firmware EQ dB table entry; `frequency` is its Hz entry.
/// Unused physical arguments can be zero. Percentages describe control position.
pub fn format_value(mode: u32, raw: u32, gain: f32, frequency: f32) -> [u16; 24] {
    let raw = raw.min(127);
    let mut text = Text::new();
    match mode {
        0 | 2 | 3 => {
            let scaled = gain * 100.0;
            let signed = (scaled + if scaled < 0.0 { -0.5 } else { 0.5 }) as i32;
            if signed < 0 {
                text.push(b'-');
            } else if signed > 0 {
                text.push(b'+');
            }
            text.decimal(signed.unsigned_abs());
            text.text(b" dB");
        }
        1 => {
            if frequency < 1000.0 {
                text.number((frequency + 0.5) as u32);
                text.text(b" Hz");
            } else {
                text.decimal((frequency / 10.0 + 0.5) as u32);
                text.text(b" kHz");
            }
        }
        8 => {
            if (63..=64).contains(&raw) {
                text.text(b"CENTER");
            } else {
                let distance = if raw < 63 {
                    text.text(b"L ");
                    63 - raw
                } else {
                    text.text(b"R ");
                    raw - 64
                };
                text.number((distance * 100 + 31) / 63);
                text.push(b'%');
            }
        }
        _ => {
            text.number((raw * 100 + 63) / 127);
            text.push(b'%');
        }
    }
    text.out
}
#[cfg(test)]
mod tests {
    use super::*;
    fn value(mode: u32, raw: u32, gain: f32, freq: f32) -> String {
        String::from_utf16(
            &format_value(mode, raw, gain, freq)
                .into_iter()
                .take_while(|c| *c != 0)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    #[test]
    fn eq_units_and_signed_rounding() {
        assert_eq!(value(0, 0, -15.0, 0.0), "-15.00 dB");
        assert_eq!(value(2, 64, 0.0, 0.0), "0.00 dB");
        assert_eq!(value(3, 65, 0.24, 0.0), "+0.24 dB");
        assert_eq!(value(0, 63, -0.23, 0.0), "-0.23 dB");
        assert_eq!(value(0, 127, 15.0, 0.0), "+15.00 dB");
    }
    #[test]
    fn frequency_units_and_rounding() {
        assert_eq!(value(1, 0, 0.0, 100.0), "100 Hz");
        assert_eq!(value(1, 64, 0.0, 815.1), "815 Hz");
        assert_eq!(value(1, 80, 0.0, 1456.1), "1.46 kHz");
        assert_eq!(value(1, 127, 0.0, 8000.0), "8.00 kHz");
    }
    #[test]
    fn percentages_are_control_positions() {
        for mode in [4, 5, 6, 7, 9] {
            assert_eq!(value(mode, 0, 0.0, 0.0), "0%");
            assert_eq!(value(mode, 64, 0.0, 0.0), "50%");
            assert_eq!(value(mode, 127, 0.0, 0.0), "100%");
        }
    }
    #[test]
    fn pan_endpoints_plateau_and_near_center() {
        assert_eq!(value(8, 0, 0.0, 0.0), "L 100%");
        assert_eq!(value(8, 62, 0.0, 0.0), "L 2%");
        assert_eq!(value(8, 63, 0.0, 0.0), "CENTER");
        assert_eq!(value(8, 64, 0.0, 0.0), "CENTER");
        assert_eq!(value(8, 65, 0.0, 0.0), "R 2%");
        assert_eq!(value(8, 127, 0.0, 0.0), "R 100%");
    }
}
