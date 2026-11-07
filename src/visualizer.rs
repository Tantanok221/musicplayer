use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub const WAVEFORM_BARS: usize = 64;

/// A short history of peaks from samples actually consumed by the audio output.
pub struct Waveform {
    peaks: [AtomicU32; WAVEFORM_BARS],
    written: AtomicUsize,
}
impl Default for Waveform {
    fn default() -> Self {
        Self {
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            written: AtomicUsize::new(0),
        }
    }
}
impl Waveform {
    pub fn push(&self, peak: f32) {
        let written = self.written.load(Ordering::Relaxed);
        self.peaks[written % WAVEFORM_BARS].store(peak.clamp(0., 1.).to_bits(), Ordering::Relaxed);
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
    }

    pub fn reset(&self) {
        for peak in &self.peaks {
            peak.store(0, Ordering::Relaxed);
        }
        self.written.store(0, Ordering::Release);
    }

    pub fn snapshot(&self) -> [f32; WAVEFORM_BARS] {
        let written = self.written.load(Ordering::Acquire);
        let count = written.min(WAVEFORM_BARS);
        std::array::from_fn(|index| {
            if index < WAVEFORM_BARS - count {
                0.
            } else {
                let slot = written.wrapping_sub(WAVEFORM_BARS - index) % WAVEFORM_BARS;
                f32::from_bits(self.peaks[slot].load(Ordering::Relaxed))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_history_keeps_recent_peaks_in_order_and_resets() {
        let waveform = Waveform::default();
        assert_eq!(waveform.snapshot(), [0.; WAVEFORM_BARS]);
        for index in 1..=80 {
            waveform.push(index as f32 / 100.);
        }
        let values = waveform.snapshot();
        assert_eq!(values[0], 0.17);
        assert_eq!(values[63], 0.8);
        waveform.reset();
        waveform.push(0.5);
        let values = waveform.snapshot();
        assert_eq!(values[0], 0.);
        assert_eq!(values[63], 0.5);
    }
}
