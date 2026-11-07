use std::{
    f32::consts::PI,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

pub const FFT_SIZE: usize = 4096;
pub const SPECTRUM_BANDS: usize = 48;
pub const LOWEST_HZ: f32 = 32.;
pub const HIGHEST_HZ: f32 = 16_000.;
const TAP_SIZE: usize = FFT_SIZE * 2;
const FLOOR_DB: f32 = -66.;
const RANGE_DB: f32 = 60.;

/// The most recent mono samples actually consumed by the audio output.
pub struct SampleTap {
    samples: [AtomicU32; TAP_SIZE],
    written: AtomicUsize,
    rate: AtomicU32,
}
impl Default for SampleTap {
    fn default() -> Self {
        Self {
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
            written: AtomicUsize::new(0),
            rate: AtomicU32::new(0),
        }
    }
}
impl SampleTap {
    pub fn push(&self, sample: f32) {
        let written = self.written.load(Ordering::Relaxed);
        self.samples[written % TAP_SIZE].store(sample.to_bits(), Ordering::Relaxed);
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        for sample in &self.samples {
            sample.store(0, Ordering::Relaxed);
        }
        self.written.store(0, Ordering::Release);
    }

    /// Copies the latest window, oldest first, and returns the sample rate.
    pub fn latest(&self, out: &mut [f32; FFT_SIZE]) -> u32 {
        let written = self.written.load(Ordering::Acquire);
        let count = written.min(FFT_SIZE);
        for (index, value) in out.iter_mut().enumerate() {
            *value = if index < FFT_SIZE - count {
                0.
            } else {
                let slot = written.wrapping_sub(FFT_SIZE - index) % TAP_SIZE;
                f32::from_bits(self.samples[slot].load(Ordering::Relaxed))
            };
        }
        self.rate.load(Ordering::Relaxed)
    }
}

/// Center frequency of a log-spaced band.
pub fn band_center(band: usize) -> f32 {
    band_edge(band as f32 + 0.5)
}

/// Horizontal position of `hz` on the analyzer, from 0 at the left to 1.
pub fn frequency_position(hz: f32) -> f32 {
    ((hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln()).clamp(0., 1.)
}

fn band_edge(band: f32) -> f32 {
    LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(band / SPECTRUM_BANDS as f32)
}

/// Log-spaced band levels with analyzer-style ballistics: bars rise almost at
/// once, fall slowly, and leave peak caps that hold before dropping.
pub struct Spectrum {
    levels: [f32; SPECTRUM_BANDS],
    peaks: [f32; SPECTRUM_BANDS],
    hold: [f32; SPECTRUM_BANDS],
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}
impl Default for Spectrum {
    fn default() -> Self {
        Self {
            levels: [0.; SPECTRUM_BANDS],
            peaks: [0.; SPECTRUM_BANDS],
            hold: [0.; SPECTRUM_BANDS],
            window: (0..FFT_SIZE)
                .map(|i| 0.5 - 0.5 * (2. * PI * i as f32 / FFT_SIZE as f32).cos())
                .collect(),
            re: vec![0.; FFT_SIZE],
            im: vec![0.; FFT_SIZE],
        }
    }
}
impl Spectrum {
    pub fn levels(&self) -> &[f32; SPECTRUM_BANDS] {
        &self.levels
    }

    pub fn peaks(&self) -> &[f32; SPECTRUM_BANDS] {
        &self.peaks
    }

    /// Advances by `dt` seconds. `None` means silence, so bars settle to zero.
    /// Returns whether anything is still moving.
    pub fn update(&mut self, input: Option<(&[f32; FFT_SIZE], u32)>, dt: f32) -> bool {
        let targets = input
            .filter(|(_, rate)| *rate > 0)
            .map_or([0.; SPECTRUM_BANDS], |(samples, rate)| {
                self.analyze(samples, rate)
            });
        let dt = dt.clamp(0., 0.1);
        for (band, target) in targets.into_iter().enumerate() {
            let level = &mut self.levels[band];
            *level = if target > *level {
                *level + (target - *level) * (dt * 30.).min(1.)
            } else {
                (*level - dt * 1.4).max(target)
            };
            if *level >= self.peaks[band] {
                self.peaks[band] = *level;
                self.hold[band] = 0.5;
            } else if self.hold[band] > 0. {
                self.hold[band] -= dt;
            } else {
                self.peaks[band] = (self.peaks[band] - dt * 0.8).max(*level);
            }
        }
        input.is_some() || self.peaks.iter().any(|peak| *peak > 0.001)
    }

    fn analyze(&mut self, samples: &[f32; FFT_SIZE], rate: u32) -> [f32; SPECTRUM_BANDS] {
        for (i, (sample, weight)) in samples.iter().zip(&self.window).enumerate() {
            self.re[i] = sample * weight;
            self.im[i] = 0.;
        }
        fft(&mut self.re, &mut self.im);
        // A full-scale sine reads 1.0 after removing the Hann window's gain.
        let scale = 4. / FFT_SIZE as f32;
        let magnitude = |re: &[f32], im: &[f32], bin: usize| {
            (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() * scale
        };
        let hz_per_bin = rate as f32 / FFT_SIZE as f32;
        let last_bin = FFT_SIZE / 2 - 1;
        std::array::from_fn(|band| {
            let low = band_edge(band as f32) / hz_per_bin;
            let high = band_edge(band as f32 + 1.) / hz_per_bin;
            let first = low.ceil() as usize;
            let last = (high.ceil() as usize).min(last_bin + 1);
            let peak = if first < last {
                (first..last)
                    .map(|bin| magnitude(&self.re, &self.im, bin))
                    .fold(0., f32::max)
            } else {
                let center = ((low + high) / 2.).min(last_bin as f32);
                let below = center.floor() as usize;
                let above = (below + 1).min(last_bin);
                let t = center - below as f32;
                magnitude(&self.re, &self.im, below) * (1. - t)
                    + magnitude(&self.re, &self.im, above) * t
            };
            // Music loses energy with frequency; tilt +3 dB per octave so
            // cymbals and vocals register beside the bass.
            let tilt = 3. * (band_center(band) / 1000.).log2();
            let db = 20. * peak.max(1e-9).log10() + tilt;
            ((db - FLOOR_DB) / RANGE_DB).clamp(0., 1.)
        })
    }
}

/// In-place iterative radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2. * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wi, wr) = (angle * k as f64).sin_cos();
                let (wr, wi) = (wr as f32, wi as f32);
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amplitude: f32, rate: u32) -> [f32; FFT_SIZE] {
        std::array::from_fn(|i| amplitude * (2. * PI * hz * i as f32 / rate as f32).sin())
    }

    fn settle(spectrum: &mut Spectrum, samples: &[f32; FFT_SIZE], rate: u32) {
        for _ in 0..30 {
            spectrum.update(Some((samples, rate)), 1. / 60.);
        }
    }

    fn loudest(spectrum: &Spectrum) -> usize {
        (0..SPECTRUM_BANDS)
            .max_by(|&a, &b| spectrum.levels()[a].total_cmp(&spectrum.levels()[b]))
            .unwrap()
    }

    #[test]
    fn tap_keeps_recent_samples_in_order_and_resets() {
        let tap = SampleTap::default();
        tap.set_rate(44_100);
        let mut out = [1.; FFT_SIZE];
        assert_eq!(tap.latest(&mut out), 44_100);
        assert_eq!(out, [0.; FFT_SIZE]);
        for index in 0..FFT_SIZE + 10 {
            tap.push(index as f32);
        }
        tap.latest(&mut out);
        assert_eq!(out[0], 10.);
        assert_eq!(out[FFT_SIZE - 1], (FFT_SIZE + 9) as f32);
        tap.reset();
        tap.push(0.5);
        tap.latest(&mut out);
        assert_eq!(out[FFT_SIZE - 2], 0.);
        assert_eq!(out[FFT_SIZE - 1], 0.5);
    }

    #[test]
    fn tones_light_the_band_for_their_frequency() {
        for hz in [60., 440., 1_000., 6_000.] {
            let mut spectrum = Spectrum::default();
            settle(&mut spectrum, &sine(hz, 0.5, 48_000), 48_000);
            let band = loudest(&spectrum);
            let center = band_center(band);
            assert!(
                (center / hz).log2().abs() < 0.25,
                "{hz} Hz peaked at band {band} ({center} Hz)"
            );
            assert!(spectrum.levels()[band] > 0.6);
            let far = (band + SPECTRUM_BANDS / 2) % SPECTRUM_BANDS;
            assert!(spectrum.levels()[far] < spectrum.levels()[band] * 0.5);
        }
    }

    #[test]
    fn silence_decays_to_rest_with_peak_caps_trailing() {
        let mut spectrum = Spectrum::default();
        settle(&mut spectrum, &sine(1_000., 0.5, 44_100), 44_100);
        let band = loudest(&spectrum);
        spectrum.update(None, 0.2);
        assert!(spectrum.peaks()[band] > spectrum.levels()[band]);
        let mut moving = true;
        for _ in 0..200 {
            moving = spectrum.update(None, 1. / 30.);
        }
        assert!(!moving);
        assert!(spectrum.levels().iter().all(|level| *level == 0.));
    }
}
