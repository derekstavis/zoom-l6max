//! The panel MCU's RTC backup domain, as persisted in `panel-rtc.bin`.
//!
//! Offsets `0x00–0x5f` hold little-endian RTC registers (`0x40002800+`) and
//! `0x80` holds RCC BDCR. Register layout follows the STM32G0 reference
//! manual (RM0444). The main firmware's Date/Time check uses CR bit 18 as
//! its calendar-valid marker.

use std::fmt;

pub const RTC_SIZE: usize = 0x100;
const TR: usize = 0x00;
const DR: usize = 0x04;
const ICSR: usize = 0x0c;
const PRER: usize = 0x10;
const WUTR: usize = 0x14;
const CR: usize = 0x18;
const BDCR: usize = 0x80;
const CALENDAR_VALID: u32 = 1 << 18;
/// BDCR after the firmware's clock bring-up in emulated L6max state: LSE on
/// and ready, medium-high drive, LSE selected as RTC clock, RTC enabled.
const BDCR_LSE_RTC_ENABLED: u32 = 0x8113;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Parse `YYYY-MM-DDTHH:MM:SS` (a space may replace `T`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid date/time {text:?}; use YYYY-MM-DDTHH:MM:SS");
        let (date, time) = text.split_once(['T', ' ']).ok_or_else(invalid)?;
        let date: Vec<_> = date.split('-').collect();
        let time: Vec<_> = time.split(':').collect();
        if date.len() != 3 || time.len() != 3 {
            return Err(invalid());
        }
        let number = |s: &str| s.parse::<u16>().map_err(|_| invalid());
        let value = Self {
            year: number(date[0])?,
            month: number(date[1])? as u8,
            day: number(date[2])? as u8,
            hour: number(time[0])? as u8,
            minute: number(time[1])? as u8,
            second: number(time[2])? as u8,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), String> {
        let leap = self.year % 4 == 0;
        let days = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return Err(format!("{self}: month is outside 1..=12")),
        };
        if !(2000..=2099).contains(&self.year) {
            return Err(format!("{self}: the RTC stores years 2000..=2099"));
        }
        if !(1..=days).contains(&self.day) || self.hour > 23 || self.minute > 59 || self.second > 59
        {
            return Err(format!("{self}: invalid date or time"));
        }
        Ok(())
    }

    /// ISO weekday as the RTC encodes it: Monday 1 through Sunday 7.
    fn weekday(&self) -> u32 {
        const T: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let mut y = u32::from(self.year);
        if self.month < 3 {
            y -= 1;
        }
        let sunday_zero =
            (y + y / 4 - y / 100 + y / 400 + T[self.month as usize - 1] + u32::from(self.day)) % 7;
        if sunday_zero == 0 { 7 } else { sunday_zero }
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

fn bcd(value: u8) -> u32 {
    u32::from(value / 10) << 4 | u32::from(value % 10)
}

fn from_bcd(value: u32) -> u8 {
    ((value >> 4 & 0xf) * 10 + (value & 0xf)) as u8
}

#[derive(Clone)]
pub struct PanelRtc {
    bytes: [u8; RTC_SIZE],
}

impl PanelRtc {
    /// The backup domain QEMU creates for a fresh device.
    pub fn seed() -> Self {
        let mut rtc = Self {
            bytes: [0; RTC_SIZE],
        };
        rtc.set(DR, 0x00266930);
        rtc.set(ICSR, 1 << 4 | 1 << 5);
        rtc.set(PRER, 0x007f00ff);
        rtc.set(WUTR, 0xffff);
        rtc
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self {
            bytes: bytes
                .try_into()
                .map_err(|_| format!("panel RTC must contain {RTC_SIZE} bytes"))?,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn get(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
    }

    fn set(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Program a valid 24-hour calendar, as the firmware's Date/Time editor does.
    pub fn set_calendar(&mut self, time: DateTime) {
        self.set(
            TR,
            bcd(time.hour) << 16 | bcd(time.minute) << 8 | bcd(time.second),
        );
        let year = (time.year - 2000) as u8;
        self.set(
            DR,
            bcd(year) << 16 | time.weekday() << 13 | bcd(time.month) << 8 | bcd(time.day),
        );
        self.set(CR, self.get(CR) | CALENDAR_VALID);
        self.set(BDCR, BDCR_LSE_RTC_ENABLED);
    }

    /// The stored calendar when the firmware's valid marker is set.
    pub fn calendar(&self) -> Option<DateTime> {
        if self.get(CR) & CALENDAR_VALID == 0 {
            return None;
        }
        let (tr, dr) = (self.get(TR), self.get(DR));
        Some(DateTime {
            year: 2000 + u16::from(from_bcd(dr >> 16 & 0xff)),
            month: from_bcd(dr >> 8 & 0x1f),
            day: from_bcd(dr & 0x3f),
            hour: from_bcd(tr >> 16 & 0x3f),
            minute: from_bcd(tr >> 8 & 0x7f),
            second: from_bcd(tr & 0x7f),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_matches_firmware_written_registers() {
        // TR/DR/CR/BDCR observed in an emulated L6max after its Date/Time setup.
        let mut rtc = PanelRtc::seed();
        rtc.set_calendar(DateTime::parse("2026-10-15T18:30:55").unwrap());
        assert_eq!(rtc.get(TR), 0x00183055);
        assert_eq!(rtc.get(DR), 0x00269015);
        assert_eq!(rtc.get(CR), 0x00040000);
        assert_eq!(rtc.get(BDCR), 0x8113);
        assert_eq!(rtc.calendar().unwrap().to_string(), "2026-10-15T18:30:55");
    }

    #[test]
    fn seed_matches_qemu_and_is_not_valid() {
        let rtc = PanelRtc::seed();
        assert_eq!(rtc.get(DR), 0x00266930);
        assert_eq!(rtc.calendar(), None);
    }

    #[test]
    fn invalid_dates_are_rejected() {
        assert!(DateTime::parse("2026-02-29T00:00:00").is_err());
        assert!(DateTime::parse("1999-01-01T00:00:00").is_err());
        assert!(DateTime::parse("2026-01-01T24:00:00").is_err());
        assert!(DateTime::parse("2028-02-29 12:00:00").is_ok());
    }
}
