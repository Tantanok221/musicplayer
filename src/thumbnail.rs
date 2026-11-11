//! Small copies of cached artwork, so lists and grids don't decode and upload a
//! 512px cover (1 MB in memory, again on the GPU) for a tile a fraction of that size.
use anyhow::{Context, Result};
use image::{DynamicImage, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType};
use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
};

/// Pixel sides of the copies, smallest first.
pub const SIZES: [u32; 2] = [128, 320];
/// Covers are drawn at up to this many device pixels per point.
const DISPLAY_SCALE: f32 = 2.;
const QUALITY: u8 = 85;

pub fn thumb_path(art: &Path, size: u32) -> PathBuf {
    art.with_extension(format!("thumb{size}.jpg"))
}

/// The smallest copy that stays sharp at `side` points, or `None` for the original.
pub fn size_for(side: f32) -> Option<u32> {
    let needed = side * DISPLAY_SCALE;
    SIZES.into_iter().find(|size| *size as f32 >= needed)
}

pub fn ready(art: &Path) -> bool {
    SIZES.iter().all(|size| thumb_path(art, *size).exists())
}

/// Writes any missing copies beside `art`, decoding it at most once.
pub fn ensure(art: &Path) -> Result<()> {
    let mut source: Option<DynamicImage> = None;
    for size in SIZES {
        let path = thumb_path(art, size);
        if path.exists() {
            continue;
        }
        if source.is_none() {
            source = Some(
                ImageReader::open(art)?
                    .with_guessed_format()?
                    .decode()
                    .context("Artwork is not a readable image")?,
            );
        }
        let image = source.as_ref().expect("decoded above");
        // Crop to a centered square like the UI's cover fit, then scale.
        let thumb = image
            .resize_to_fill(size, size, FilterType::CatmullRom)
            .to_rgb8();
        // Unique so concurrent lookups of one cover never share a partial file.
        let partial = path.with_extension(format!("partial{}.jpg", rand::random::<u32>()));
        let mut out = BufWriter::new(File::create(&partial)?);
        thumb.write_with_encoder(JpegEncoder::new_with_quality(&mut out, QUALITY))?;
        out.into_inner()
            .map_err(|error| error.into_error())?
            .sync_all()?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(())
}

/// Copies derived from `art` that must be removed with it.
pub fn derived(art: &Path) -> Vec<PathBuf> {
    SIZES.iter().map(|size| thumb_path(art, *size)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn picks_the_smallest_sharp_copy_or_the_original() {
        assert_eq!(size_for(34.), Some(128));
        assert_eq!(size_for(64.), Some(128));
        assert_eq!(size_for(124.), Some(320));
        assert_eq!(size_for(160.), Some(320));
        assert_eq!(size_for(161.), None);
    }

    #[test]
    fn writes_square_copies_once() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("cover.art");
        RgbImage::from_pixel(600, 400, Rgb([200, 40, 40]))
            .save_with_format(&art, image::ImageFormat::Png)
            .unwrap();
        assert!(!ready(&art));
        ensure(&art).unwrap();
        assert!(ready(&art));
        for size in SIZES {
            let thumb = image::open(thumb_path(&art, size)).unwrap();
            assert_eq!((thumb.width(), thumb.height()), (size, size));
        }
        let written = std::fs::metadata(thumb_path(&art, 128))
            .unwrap()
            .modified()
            .unwrap();
        ensure(&art).unwrap();
        let again = std::fs::metadata(thumb_path(&art, 128))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(written, again);
    }

    #[test]
    fn unreadable_art_fails_without_leaving_copies() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("broken.art");
        std::fs::write(&art, b"not an image").unwrap();
        assert!(ensure(&art).is_err());
        assert!(!ready(&art));
    }
}
