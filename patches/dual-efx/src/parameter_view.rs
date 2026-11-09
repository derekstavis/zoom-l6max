//! Copied parameter values and display formatting, independent of DSP state.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EfxParamId {
    // Velvet Noise Decorrelator (Effect A)
    VndAmount = 0,
    VndTimeSpread = 1,
    VndDensity = 2,
    VndDecay = 3,
    VndRefresh = 4,
    VndMode = 5,

    // Plateau Plate Reverb (Effect B)
    PlatPreDelay = 6,
    PlatSize = 7,
    PlatDecay = 8,
    PlatLowCut = 9,
    PlatHighCut = 10,
    PlatDiffusion = 11,
    PlatModDepth = 12,
    PlatFreeze = 13,

    // Global / Routing
    GlobalCascade = 14,
    // Keep the earlier discriminants stable.
    VndTransition = 15,
}

impl EfxParamId {
    /// Decodes a copied event without constructing an invalid enum value.
    pub fn from_index(index: u32) -> Option<Self> {
        use EfxParamId::*;
        Some(match index {
            0 => VndAmount, 1 => VndTimeSpread, 2 => VndDensity, 3 => VndDecay,
            4 => VndRefresh, 5 => VndMode, 6 => PlatPreDelay, 7 => PlatSize,
            8 => PlatDecay, 9 => PlatLowCut, 10 => PlatHighCut, 11 => PlatDiffusion,
            12 => PlatModDepth, 13 => PlatFreeze, 14 => GlobalCascade, 15 => VndTransition,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParameterSnapshot {
    pub id: EfxParamId,
    /// Numeric value in displayed units; booleans are 0.0 or 1.0.
    pub value: f32,
}

impl ParameterSnapshot {
    /// Decodes the two words used to transfer a value to the display task.
    pub fn from_words(index: u32, bits: u32) -> Option<Self> {
        let id = EfxParamId::from_index(index)?;
        let value = f32::from_bits(bits);
        use EfxParamId::*;
        // Bounds are in displayed units and must track the DSP setters and
        // snapshot scaling, including f32 rounding at the endpoints.
        let (min, max) = match id {
            VndAmount | PlatDiffusion | GlobalCascade => (0.0, 100.0),
            VndTimeSpread => (1.0, 100.0),
            // Rounding a valid 4000 pps request to whole taps can yield a
            // slightly higher effective density at short spreads.
            VndDensity => (10.0, 4600.0),
            VndDecay => (-120.0, 0.0),
            VndRefresh => (0.1, 60.0),
            VndTransition => (1.0, 54000.0),
            PlatPreDelay => (0.0, if cfg!(experimental_plateau_32k) { 500.0 } else { 190.0 }),
            PlatSize => (0.01f32 * 100.0, if cfg!(experimental_plateau_32k) { 400.0 } else { 1.2f32 * 100.0 }),
            PlatDecay => (0.1f32 * 100.0, 0.999f32 * 100.0),
            PlatLowCut | PlatHighCut => (20.0, if cfg!(experimental_plateau_32k) { 32000.0 * 0.49 } else { 48000.0 * 0.49 }),
            PlatModDepth => (0.0, 16.0),
            VndMode | PlatFreeze => {
                return (value == 0.0 || value == 1.0).then_some(Self { id, value });
            }
        };
        if !value.is_finite() || value < min || value > max {
            return None;
        }
        Some(Self { id, value })
    }
}

/// A copied inspector value; IDs 16 through 23 name strips, 24 names the
/// page, and 25 reports a rejected turn when the input queue is full.
#[derive(Clone, Copy, Debug)]
pub enum DisplaySnapshot {
    Parameter(ParameterSnapshot),
    Route { strip: u8, mode: u8 },
    Page { routing: bool, velvet: bool },
    QueueFull,
}

impl DisplaySnapshot {
    pub fn from_words(index: u32, bits: u32) -> Option<Self> {
        if index < 16 {
            return ParameterSnapshot::from_words(index, bits).map(Self::Parameter);
        }
        if index == 24 {
            return match f32::from_bits(bits) {
                0.0 => Some(Self::Page { routing: false, velvet: false }),
                1.0 => Some(Self::Page { routing: true, velvet: false }),
                2.0 => Some(Self::Page { routing: false, velvet: true }),
                _ => None,
            };
        }
        if index == 25 {
            return (f32::from_bits(bits) == 0.0).then_some(Self::QueueFull);
        }
        let strip = index.checked_sub(16)?;
        let value = f32::from_bits(bits);
        if strip >= 8 || ![0.0, 1.0, 2.0, 3.0].contains(&value) {
            return None;
        }
        Some(Self::Route { strip: strip as u8, mode: value as u8 })
    }
}

pub struct FormattedParam {
    pub title: &'static str,
    pub value_str: [u8; 16],
    pub value_len: usize,
}

impl FormattedParam {
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            value_str: [0; 16],
            value_len: 0,
        }
    }
}

/// Helper to write an integer into a byte buffer.
fn format_int(buf: &mut [u8], mut val: i32) -> usize {
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let negative = val < 0;
    if negative {
        val = -val;
    }
    let mut temp = [0u8; 10];
    let mut i = 0;
    while val > 0 {
        temp[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }
    let mut out_len = 0;
    if negative {
        buf[out_len] = b'-';
        out_len += 1;
    }
    while i > 0 {
        i -= 1;
        buf[out_len] = temp[i];
        out_len += 1;
    }
    out_len
}

/// Helper to write a float with 1 decimal place.
fn format_float_1dp(buf: &mut [u8], val: f32) -> usize {
    let negative = val < 0.0;
    let magnitude = if negative { -val } else { val };
    let scaled = (magnitude * 10.0 + 0.5) as i32;
    let mut len = 0;
    if negative && scaled != 0 {
        buf[0] = b'-';
        len = 1;
    }
    len += format_int(&mut buf[len..], scaled / 10);
    if len + 2 <= buf.len() {
        buf[len] = b'.';
        buf[len + 1] = b'0' + (scaled % 10) as u8;
        len += 2;
    }
    len
}

fn append_str(buf: &mut [u8], start: usize, s: &[u8]) -> usize {
    let mut idx = start;
    for &b in s {
        if idx >= buf.len() {
            break;
        }
        buf[idx] = b;
        idx += 1;
    }
    idx
}

/// Formats a copied parameter or strip routing value on the display task.
pub fn format_display(snapshot: DisplaySnapshot) -> FormattedParam {
    match snapshot {
        DisplaySnapshot::Parameter(parameter) => format_snapshot(parameter),
        DisplaySnapshot::QueueFull => {
            let mut formatted = FormattedParam::new("EFX INPUT");
            formatted.value_len = append_str(&mut formatted.value_str, 0, b"BUSY - RETRY");
            formatted
        }
        DisplaySnapshot::Page { routing, velvet } => {
            let mut formatted = FormattedParam::new("EFX CONTROLS");
            let value: &[u8] = if routing { b"ROUTING" } else if velvet { b"VELVET PARAM" } else { b"PLATEAU PARAM" };
            formatted.value_len = append_str(&mut formatted.value_str, 0, value);
            formatted
        }
        DisplaySnapshot::Route { strip, mode } => {
            let titles = ["CH 1 ROUTE", "CH 2 ROUTE", "CH 3 ROUTE", "CH 4 ROUTE",
                          "CH 5 ROUTE", "CH 6 ROUTE", "CH 7 ROUTE", "CH 8 ROUTE"];
            let values: [&[u8]; 4] = [b"OFF", b"A", b"B", b"A -> B"];
            let mut formatted = FormattedParam::new(titles.get(strip as usize).copied().unwrap_or("ROUTE"));
            formatted.value_len = append_str(&mut formatted.value_str, 0, values.get(mode as usize).copied().unwrap_or(b"?"));
            formatted
        }
    }
}

/// Formats a previously copied value without accessing the live processor.
pub fn format_snapshot(snapshot: ParameterSnapshot) -> FormattedParam {
    use EfxParamId::*;
    let (title, unit): (&str, &[u8]) = match snapshot.id {
        VndAmount => ("VND AMOUNT", b" %"),
        VndTimeSpread => ("VND TIME SPREAD", b" ms"),
        VndDensity => ("VND DENSITY", b" pps"),
        VndDecay => ("VND DECAY", b" dB"),
        VndRefresh => ("VND REFRESH", b" s"),
        VndTransition => ("VND TRANSITION", b" ms"),
        VndMode => {
            let mut p = FormattedParam::new("VND MODE");
            p.value_len = append_str(&mut p.value_str, 0, if snapshot.value != 0.0 { b"DYNAMIC" } else { b"STATIC" });
            return p;
        }
        PlatPreDelay => ("PLATEAU PREDELAY", b" ms"),
        PlatSize => ("PLATEAU SIZE", b" %"),
        PlatDecay => ("PLATEAU DECAY", b" %"),
        PlatLowCut => ("PLATEAU LOW CUT", b" Hz"),
        PlatHighCut => ("PLATEAU HIGH CUT", b" Hz"),
        PlatDiffusion => ("PLATEAU DIFFUSION", b" %"),
        PlatModDepth => ("PLATEAU MOD DEPTH", b" samples"),
        PlatFreeze => {
            let mut p = FormattedParam::new("PLATEAU FREEZE");
            p.value_len = append_str(&mut p.value_str, 0, if snapshot.value != 0.0 { b"FROZEN" } else { b"OFF" });
            return p;
        }
        GlobalCascade => ("A -> B CASCADE", b" %"),
    };
    let mut p = FormattedParam::new(title);
    let len = format_float_1dp(&mut p.value_str, snapshot.value);
    p.value_len = append_str(&mut p.value_str, len, unit);
    p
}
