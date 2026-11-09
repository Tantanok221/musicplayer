//! Negotiate the current output independently of the original decoded format.
use super::AudioFormat;
use anyhow::Result;
use flume::Sender;
use rodio::{
    Source,
    cpal::{self, traits::DeviceTrait},
};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::time::Duration;

fn select_config(
    source: AudioFormat,
    default: &cpal::SupportedStreamConfig,
    supported: &[cpal::SupportedStreamConfigRange],
) -> cpal::SupportedStreamConfig {
    // Prefer the original rate and channels, then adapt channels without changing
    // rate. Otherwise the device's default is the most reliable shared output.
    for channels in [source.channels.get(), default.channels()] {
        if let Some(config) = supported
            .iter()
            .filter(|c| c.channels() == channels)
            .filter_map(|c| c.try_with_sample_rate(source.rate.get()))
            .min_by_key(|c| c.sample_format() != default.sample_format())
        {
            return config;
        }
    }
    *default
}

#[cfg(target_os = "macos")]
fn device_buffer(config: &cpal::SupportedStreamConfig) -> cpal::BufferSize {
    match *config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } if min <= max && max > 0 => {
            let preferred = (config.sample_rate() / 25).max(1).next_power_of_two();
            cpal::BufferSize::Fixed(preferred.clamp(min.max(1), max))
        }
        _ => cpal::BufferSize::Default,
    }
}

pub(super) fn open_device(
    device: &cpal::Device,
    source: AudioFormat,
    errors: Sender<cpal::Error>,
) -> Result<rodio::MixerDeviceSink> {
    let default = device.default_output_config()?;
    let supported = device
        .supported_output_configs()
        .map(|configs| configs.collect::<Vec<_>>())
        .unwrap_or_default();
    let config = select_config(source, &default, &supported);
    let build = |config: &cpal::SupportedStreamConfig| {
        let builder =
            rodio::DeviceSinkBuilder::from_device(device.clone())?.with_supported_config(config);
        // Stay within the device's range and leave enough time for conversion.
        #[cfg(target_os = "macos")]
        let builder = builder.with_buffer_size(device_buffer(config));
        let errors = errors.clone();
        builder
            .with_error_callback(move |error| {
                let _ = errors.send(error);
            })
            .open_stream()
    };
    let output = build(&config).or_else(|error| {
        if config.channels() == default.channels()
            && config.sample_rate() == default.sample_rate()
            && config.sample_format() == default.sample_format()
        {
            Err(error)
        } else {
            // Remain on the selected device; never fall back to another speaker.
            build(&default)
        }
    })?;
    Ok(output)
}

pub(super) fn adapt<S: Source + Send + 'static>(
    source: S,
    target_rate: rodio::SampleRate,
) -> Result<Box<dyn Source + Send>> {
    if source.sample_rate() == target_rate {
        Ok(Box::new(source))
    } else {
        Ok(Box::new(ResampledAudio::new(source, target_rate)?))
    }
}

/// A bounded, preallocated sinc converter. Rodio's channel adapter can still
/// duplicate mono into stereo, but its unfiltered rate converter is bypassed.
struct ResampledAudio<S> {
    source: S,
    resampler: SincFixedIn<f32>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    source_rate: rodio::SampleRate,
    target_rate: rodio::SampleRate,
    channels: rodio::ChannelCount,
    duration: Option<Duration>,
    frames: usize,
    offset: usize,
    skip_samples: usize,
    input_frames: u64,
    emitted_samples: u64,
    ended: bool,
}
impl<S: Source> ResampledAudio<S> {
    fn new(source: S, target_rate: rodio::SampleRate) -> Result<Self> {
        let channels = source.channels();
        let source_rate = source.sample_rate();
        let duration = source.total_duration();
        let resampler = SincFixedIn::<f32>::new(
            f64::from(target_rate.get()) / f64::from(source_rate.get()),
            1.,
            SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: 128,
                window: WindowFunction::BlackmanHarris2,
            },
            256,
            usize::from(channels.get()),
        )?;
        let input = resampler.input_buffer_allocate(true);
        let output = resampler.output_buffer_allocate(true);
        let skip_samples = resampler.output_delay() * usize::from(channels.get());
        Ok(Self {
            source,
            resampler,
            input,
            output,
            source_rate,
            target_rate,
            channels,
            duration,
            frames: 0,
            offset: 0,
            skip_samples,
            input_frames: 0,
            emitted_samples: 0,
            ended: false,
        })
    }
    fn target_samples(&self) -> u64 {
        (self.input_frames * u64::from(self.target_rate.get()))
            .div_ceil(u64::from(self.source_rate.get()))
            * u64::from(self.channels.get())
    }
    fn fill(&mut self) -> bool {
        for channel in &mut self.input {
            channel.fill(0.);
        }
        if !self.ended {
            for frame in 0..self.resampler.input_frames_next() {
                let Some(first) = self.source.next() else {
                    self.ended = true;
                    break;
                };
                self.input[0][frame] = first;
                for channel in self.input.iter_mut().skip(1) {
                    channel[frame] = self.source.next().unwrap_or(0.);
                }
                self.input_frames += 1;
            }
        }
        if self.ended && self.emitted_samples >= self.target_samples() {
            return false;
        }
        // Both buffers are allocated from this fixed-size resampler and never resized.
        let (_, frames) = self
            .resampler
            .process_into_buffer(&self.input, &mut self.output, None)
            .expect("preallocated resampler buffers retain their required sizes");
        self.frames = frames;
        self.offset = 0;
        true
    }
}
impl<S: Source> Iterator for ResampledAudio<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        loop {
            if self.ended && self.emitted_samples >= self.target_samples() {
                return None;
            }
            let channels = usize::from(self.channels.get());
            if self.offset >= self.frames * channels && !self.fill() {
                return None;
            }
            if self.offset >= self.frames * channels {
                continue;
            }
            let sample = self.output[self.offset % channels][self.offset / channels];
            self.offset += 1;
            if self.skip_samples > 0 {
                self.skip_samples -= 1;
                continue;
            }
            self.emitted_samples += 1;
            return Some(sample);
        }
    }
}
impl<S: Source> Source for ResampledAudio<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.target_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    fn format(rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            rate: rate.try_into().unwrap(),
            channels: channels.try_into().unwrap(),
        }
    }
    fn config(rate: u32, channels: u16) -> cpal::SupportedStreamConfig {
        cpal::SupportedStreamConfig::new(
            channels,
            rate,
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )
    }
    fn range(rate: u32, channels: u16) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(
            channels,
            rate,
            rate,
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )
    }
    #[test]
    fn bluetooth_selects_supported_output_for_high_rates_and_mono() {
        let default = config(44_100, 2);
        let supported = [range(44_100, 2)];
        for rate in [
            8_000, 22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
        ] {
            for channels in [1, 2] {
                let selected = select_config(format(rate, channels), &default, &supported);
                assert_eq!((selected.sample_rate(), selected.channels()), (44_100, 2));
            }
        }
        let selected = select_config(
            format(96_000, 2),
            &default,
            &[range(44_100, 2), range(96_000, 2)],
        );
        assert_eq!(
            selected.sample_rate(),
            96_000,
            "retain native output when supported"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            device_buffer(&cpal::SupportedStreamConfig::new(
                2,
                44_100,
                cpal::SupportedBufferSize::Range { min: 14, max: 1024 },
                cpal::SampleFormat::F32
            )),
            cpal::BufferSize::Fixed(1024)
        );
    }
    #[test]
    fn native_pcm_is_unchanged_and_conversion_preserves_duration_and_channels() {
        for rate in [
            8_000, 22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
        ] {
            for channels in [1, 2] {
                let source_format = format(rate, channels);
                let samples = vec![0.125; (rate / 10) as usize * usize::from(channels)];
                let source =
                    SamplesBuffer::new(source_format.channels, source_format.rate, samples.clone());
                let converted = adapt(source, 44_100.try_into().unwrap()).unwrap();
                assert_eq!(converted.channels().get(), channels);
                assert_eq!(converted.sample_rate().get(), 44_100);
                let output = converted.collect::<Vec<_>>();
                assert_eq!(
                    output.len(),
                    4_410 * usize::from(channels),
                    "duration at {rate} Hz/{channels} channels"
                );
                assert!(output.iter().all(|s| s.is_finite()));
                if rate == 44_100 {
                    assert_eq!(output, samples);
                }
            }
        }
    }
    #[test]
    fn downsampling_filters_frequencies_above_the_speaker_nyquist_limit() {
        fn rms(frequency: f64) -> f64 {
            let samples = (0..96_000)
                .map(|frame| {
                    (2. * std::f64::consts::PI * frequency * f64::from(frame) / 96_000.).sin()
                        as f32
                        * 0.5
                })
                .collect::<Vec<_>>();
            let source =
                SamplesBuffer::new(1.try_into().unwrap(), 96_000.try_into().unwrap(), samples);
            let output = adapt(source, 44_100.try_into().unwrap())
                .unwrap()
                .collect::<Vec<_>>();
            let middle = &output[1000..output.len() - 1000];
            (middle.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / middle.len() as f64).sqrt()
        }
        assert!(rms(1_000.) > 0.3, "retain audible content");
        assert!(
            rms(30_000.) < 0.001,
            "prevent ultrasonic content aliasing into audible frequencies"
        );
    }

    #[test]
    fn common_file_formats_decode_and_convert_to_bluetooth_rate() {
        for name in [
            "tone-0.flac",
            "tone-0.mp3",
            "tone-96k.flac",
            "tone-aac.m4a",
            "tone-alac.m4a",
            "tone-96k.aiff",
            "tone-192k.wav",
            "tone-vorbis.ogg",
        ] {
            let file = std::fs::File::open(format!("tests/fixtures/{name}")).unwrap();
            let decoded = rodio::Decoder::try_from(file).unwrap();
            let rate = decoded.sample_rate();
            let channels = decoded.channels();
            let samples = decoded.collect::<Vec<_>>();
            assert!(!samples.is_empty(), "decode {name}");
            let frames = samples.len() / usize::from(channels.get());
            let source = SamplesBuffer::new(channels, rate, samples);
            let converted = adapt(source, 44_100.try_into().unwrap())
                .unwrap()
                .collect::<Vec<_>>();
            assert_eq!(
                converted.len(),
                (frames * 44_100).div_ceil(rate.get() as usize) * usize::from(channels.get()),
                "duration of {name}"
            );
            assert!(
                converted.iter().all(|s| s.is_finite()),
                "finite samples from {name}"
            );
            assert!(
                converted.iter().any(|s| s.abs() > 0.01),
                "audible samples from {name}"
            );
        }
    }
}
