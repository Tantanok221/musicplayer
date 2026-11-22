//! Chromaprint fingerprints of local files, for AcoustID lookups.
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use rodio::Source;
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};
use std::path::Path;

/// AcoustID only needs about the first two minutes.
const SECONDS: u32 = 120;

/// The compressed, base64 fingerprint AcoustID's lookup takes.
pub fn fingerprint(path: &Path) -> Result<String> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Couldn't open {}", path.display()))?;
    let decoded = rodio::Decoder::try_from(file)
        .with_context(|| format!("Couldn't decode {}", path.display()))?;
    let rate = u32::from(decoded.sample_rate());
    let channels = u32::from(decoded.channels().get());
    let config = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&config);
    printer
        .start(rate, channels)
        .map_err(|error| anyhow!("Can't fingerprint this audio: {error:?}"))?;
    let samples: Vec<i16> = decoded
        .take((rate * channels * SECONDS) as usize)
        .map(|sample| (sample.clamp(-1., 1.) * f32::from(i16::MAX)) as i16)
        .collect();
    printer.consume(&samples);
    printer.finish();
    let compressed = FingerprintCompressor::from(&config).compress(printer.fingerprint());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fingerprints_a_local_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone-0.flac");
        let print = super::fingerprint(&path).unwrap();
        assert!(!print.is_empty());
        assert!(!print.contains(['+', '/', '=']));
        assert!(super::fingerprint(Path::new("/nonexistent.flac")).is_err());
    }
    use std::path::Path;
}
