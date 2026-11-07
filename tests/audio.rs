use rodio::{Decoder, Source};
use std::{fs::File, time::Duration};

fn decode(path: &str) -> Vec<f32> {
    let file = File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    let decoder = Decoder::builder()
        .with_data(file)
        .with_byte_len(len)
        .with_seekable(true)
        .with_gapless(true)
        .build()
        .unwrap();
    assert_eq!(decoder.sample_rate().get(), 48000);
    assert_eq!(decoder.channels().get(), 2);
    decoder.collect()
}
#[test]
fn flac_fixture_retains_exact_pcm_length_and_continuity() {
    let a = decode("tests/fixtures/tone-0.flac");
    let b = decode("tests/fixtures/tone-1.flac");
    assert_eq!(a.len(), 9600);
    assert_eq!(b.len(), 9600);
    let joined = [a, b].concat();
    for frame in [4799usize, 4800, 4801] {
        let expected = (10000. * (2. * std::f64::consts::PI * 440. * frame as f64 / 48000.).sin())
            as i16 as f32
            / 32768.;
        assert!((joined[frame * 2] - expected).abs() < 0.0001);
    }
}
#[test]
fn mp3_gapless_decoder_removes_encoder_delay_and_padding() {
    let a = decode("tests/fixtures/tone-0.mp3");
    let b = decode("tests/fixtures/tone-1.mp3");
    assert_eq!(a.len(), 9600, "Encoder delay and padding must be trimmed");
    assert_eq!(b.len(), 9600);
    let joined = [a, b].concat();
    let boundary = &joined[9500..9700];
    assert!(boundary.iter().any(|v| v.abs() > 0.1));
}
#[test]
fn flac_and_mp3_are_seekable() {
    for suffix in ["flac", "mp3"] {
        let file = File::open(format!("tests/fixtures/tone-0.{suffix}")).unwrap();
        let len = file.metadata().unwrap().len();
        let mut decoder = Decoder::builder()
            .with_data(file)
            .with_byte_len(len)
            .with_seekable(true)
            .with_gapless(true)
            .build()
            .unwrap();
        decoder.try_seek(Duration::from_millis(50)).unwrap();
        assert!(decoder.next().is_some());
    }
}
