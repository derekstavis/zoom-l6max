//! Fast, standalone math functions for bare-metal ARM Cortex-M7 (`no_std`).
//! Avoids relying on libm or dynamic allocations while guaranteeing single-cycle FPU friendliness.

pub const PI: f32 = 3.14159265358979323846;
pub const PI_2: f32 = 1.57079632679489661923;
pub const TWO_PI: f32 = 6.28318530717958647692;

/// Reject corrupt samples before they enter delay lines and feedback state.
/// The limit leaves 120 dB of headroom above normalized full scale.
#[inline(always)]
pub fn audio_input(x: f32) -> f32 {
    if x >= -1_000_000.0 && x <= 1_000_000.0 { x } else { 0.0 }
}

/// Fast clamp.
#[inline(always)]
pub fn clamp(val: f32, min: f32, max: f32) -> f32 {
    if val.is_nan() || val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}

/// Continuous cubic soft-clipper to prevent harsh digital clipping and protect downstream DACs:
/// f(x) = x - (4/27)*x^3 for |x| < 1.5, clamped to [-1.0, 1.0].
/// Small-signal gain f'(0) is exactly 1.0 (unity gain).
/// Derivative f'(1.5) = 1 - 4/9*(2.25) = 0.0 (smooth C1 transition at ±1.0 ceiling).
#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else if x <= -1.5 {
        -1.0
    } else if x >= 1.5 {
        1.0
    } else {
        x - (4.0 / 27.0) * (x * x * x)
    }
}

/// Fast clamp for usize.
#[inline(always)]
pub fn clamp_usize(val: usize, min: usize, max: usize) -> usize {
    if val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}

/// Fast square root via bitwise initial estimate and two Newton iterations.
#[inline(always)]
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = x;
    let mut i = y.to_bits();
    i = 0x1fbb4000 + (i >> 1); // Fast initial estimate
    y = f32::from_bits(i);
    // Two Newton iterations
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    y
}

/// Fast sine approximation using 5th-order minimax polynomial.
/// Accurate to within 0.001 across all angles.
#[inline(always)]
pub fn sin(mut x: f32) -> f32 {
    // Reducing with repeated subtraction can run indefinitely for infinity or
    // for large floats where subtracting TWO_PI no longer changes the value.
    if !x.is_finite() {
        return 0.0;
    }
    if x > PI || x < -PI {
        x %= TWO_PI;
        if x > PI {
            x -= TWO_PI;
        } else if x < -PI {
            x += TWO_PI;
        }
    }

    // Minimax polynomial for sin(x) on [-PI, PI]
    // B = 4/pi, C = -4/pi^2
    const B: f32 = 1.27323954;
    const C: f32 = -0.405284735;
    let mut y = B * x + C * x * abs(x);
    // Extra precision smoothing: Q = 0.225 * (y * |y| - y) + y
    const P: f32 = 0.225;
    y = P * (y * abs(y) - y) + y;
    y
}

/// Fast cosine approximation: cos(x) = sin(x + PI/2).
#[inline(always)]
pub fn cos(x: f32) -> f32 {
    sin(x + PI_2)
}

/// Fast absolute value.
#[inline(always)]
pub fn abs(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7fffffff)
}

/// Fast 10^x approximation for dB gain conversion: envelope = 10^(decay_db * progress / 20.0).
/// 10^x = 2^(x * log2(10)) = 2^(x * 3.321928095).
#[inline(always)]
pub fn pow10(x: f32) -> f32 {
    exp2(x * 3.321928095)
}

/// Fast 2^x via IEEE-754 float bit-level manipulation + polynomial.
#[inline(always)]
pub fn exp2(x: f32) -> f32 {
    if x < -126.0 {
        return 0.0;
    }
    if x > 126.0 {
        return f32::MAX;
    }
    let floor_x = x as i32 - if x < (x as i32 as f32) { 1 } else { 0 };
    let frac = x - floor_x as f32;

    // Polynomial approximation of 2^frac on [0, 1]
    // 2^f ≈ 1 + f * (0.695501 + f * (0.2262698 + f * 0.0782318))
    let exp_frac = 1.0 + frac * (0.695501 + frac * (0.2262698 + frac * 0.0782318));
    let exponent_bits = ((floor_x + 127) as u32) << 23;
    let scale = f32::from_bits(exponent_bits);
    exp_frac * scale
}

/// Converts decibels to linear gain: gain = 10^(db / 20).
#[inline(always)]
pub fn db_to_gain(db: f32) -> f32 {
    pow10(db * 0.05)
}
