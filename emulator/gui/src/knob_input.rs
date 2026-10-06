//! Fractional input accumulation and axis locking for encoder gestures.
#[derive(Default)]
pub struct KnobInput {
    anchor: Option<(f32, f32)>,
    horizontal: Option<bool>,
    scroll: f32,
}
impl KnobInput {
    pub fn dragging(&self) -> bool {
        self.anchor.is_some()
    }
    pub fn start(&mut self, x: f32, y: f32) {
        self.anchor = Some((x, y));
        self.horizontal = None;
    }
    pub fn stop(&mut self) {
        self.anchor = None;
        self.horizontal = None;
    }
    pub fn drag(&mut self, x: f32, y: f32, threshold: f32) -> i32 {
        let Some((ax, ay)) = self.anchor else {
            return 0;
        };
        let horizontal = self.horizontal.unwrap_or((x - ax).abs() > (y - ay).abs());
        let delta = if horizontal { x - ax } else { ay - y };
        let scaled = delta / threshold;
        let nearest = scaled.round();
        // Positions and the scaled threshold are f32. Subtracting two window
        // coordinates can leave an exact physical step just below an integer,
        // particularly after resizing. Snap only within that roundoff bound;
        // real fractional motion is still retained in the anchor.
        let coordinate = x.abs().max(y.abs()).max(ax.abs()).max(ay.abs());
        let roundoff = 4.0 * f32::EPSILON * coordinate.max(threshold) / threshold;
        let steps = if (scaled - nearest).abs() <= roundoff {
            nearest as i32
        } else {
            scaled.trunc() as i32
        };
        if steps != 0 {
            self.horizontal = Some(horizontal);
            self.anchor = Some(if horizontal {
                (ax + steps as f32 * threshold, y)
            } else {
                (x, ay - steps as f32 * threshold)
            });
        }
        steps
    }
    pub fn scroll(&mut self, delta: f32) -> i32 {
        self.scroll += delta;
        let steps = self.scroll.trunc() as i32;
        self.scroll -= steps as f32;
        steps
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_scroll_is_retained_and_reverses() {
        let mut k = KnobInput::default();
        assert_eq!(k.scroll(0.4), 0);
        assert_eq!(k.scroll(0.4), 0);
        assert_eq!(k.scroll(0.4), 1);
        assert_eq!(k.scroll(-1.4), -1);
    }
    #[test]
    fn dragging_locks_axis_retains_remainder_and_stops() {
        let mut k = KnobInput::default();
        k.start(0., 0.);
        assert_eq!(k.drag(2., -7., 8.), 0);
        assert_eq!(k.drag(3., -19., 8.), 2);
        assert_eq!(k.drag(100., -24., 8.), 1);
        assert_eq!(k.drag(100., -8., 8.), -2);
        k.stop();
        assert_eq!(k.drag(0., 0., 8.), 0);
        k.start(0., 0.);
        assert_eq!(k.drag(24., -4., 8.), 3);
        assert_eq!(k.drag(8., -100., 8.), -2);
    }
    #[test]
    fn resized_drag_counts_exact_steps_without_counting_fractional_motion_early() {
        let scale = 1200.0 / 1095.0;
        let (x, y) = (744.0 * scale, 366.0 * scale);
        let threshold = 8.0 * scale;
        let mut k = KnobInput::default();
        k.start(x, y);
        assert_eq!(k.drag(x, y - 47.9 * scale, threshold), 5);
        assert_eq!(k.drag(x, y - 48.0 * scale, threshold), 1);
        assert_eq!(k.drag(x, y - 48.0 * scale, threshold), 0);
        assert_eq!(k.drag(x, y - 56.0 * scale, threshold), 1);
        k.stop();
        let x = 884.0 * scale;
        k.start(x, y);
        assert_eq!(k.drag(x - 32.0 * scale, y, threshold), -4);
        assert_eq!(k.drag(x - 32.0 * scale, y, threshold), 0);
    }
}
