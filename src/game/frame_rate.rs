//! Render cadence, independent of the fixed gameplay/server tick rate.
#[derive(Default)]
pub(super) struct FrameRate {
    elapsed: f64,
    frames: u32,
    measured: Option<f64>,
}

impl FrameRate {
    fn record(&mut self, seconds: f32) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        self.elapsed += f64::from(seconds);
        self.frames += 1;
        if self.elapsed >= 0.5 {
            self.measured = Some(f64::from(self.frames) / self.elapsed);
            self.elapsed = 0.0;
            self.frames = 0;
        }
    }

    pub(super) fn digits(&self) -> String {
        self.measured.map_or_else(
            || "--".to_owned(),
            |fps| {
                let value = format!("{fps:.1}");
                value.strip_suffix(".0").unwrap_or(&value).to_owned()
            },
        )
    }
}

impl super::Game {
    pub(crate) fn record_render_frame(&mut self, seconds: f32) {
        self.frame_rate.record(seconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counts_rendered_frames_instead_of_simulation_steps() {
        for hz in [30.0, 60.0, 120.0, 144.0] {
            let mut rate = FrameRate::default();
            for _ in 0..200 {
                rate.record(1.0 / hz);
            }
            assert_eq!(rate.digits(), format!("{hz:.0}"));
        }
    }

    #[test]
    fn fps_has_one_decimal_without_trailing_zero_and_uses_elapsed_time() {
        let mut rate = FrameRate::default();
        for _ in 0..100 {
            rate.record(1.0 / 50.5);
        }
        assert_eq!(rate.digits(), "50.5");
        rate.record(2.0);
        assert!(
            rate.measured.unwrap() < 25.0,
            "a real frame stall counts against cadence"
        );
        let before = rate.digits();
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            rate.record(invalid);
        }
        assert_eq!(rate.digits(), before);
    }
}
