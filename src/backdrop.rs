use anyhow::{Context, Result};
use image::{ImageReader, RgbImage, imageops, imageops::FilterType};
use std::path::{Path, PathBuf};

const SIDE: u32 = 192;
const PALETTE_SIZE: usize = 3;

/// A pre-blurred copy of album art with its prominent colors.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub path: PathBuf,
    /// 0xRRGGBB colors, most prominent first.
    pub palette: [u32; PALETTE_SIZE],
}

pub fn backdrop_path(art: &Path) -> PathBuf {
    art.with_extension("backdrop-v1.png")
}

/// Reads the cached backdrop beside `art`, rendering it first when missing.
pub fn load_or_render(art: &Path) -> Result<Backdrop> {
    let path = backdrop_path(art);
    let image = match image::open(&path) {
        Ok(image) => image.to_rgb8(),
        Err(_) => {
            let image = render(art)?;
            let partial = path.with_extension("partial.png");
            image.save(&partial)?;
            std::fs::rename(&partial, &path)?;
            image
        }
    };
    Ok(Backdrop {
        palette: palette(&image),
        path,
    })
}

fn render(art: &Path) -> Result<RgbImage> {
    let source = ImageReader::open(art)?
        .with_guessed_format()?
        .decode()
        .context("Artwork is not a readable image")?;
    let mut small = source.resize_exact(48, 48, FilterType::Triangle).to_rgb8();
    for pixel in small.pixels_mut() {
        let [r, g, b] = pixel.0.map(f32::from);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        pixel.0 = [r, g, b].map(|channel| (luma + (channel - luma) * 1.3).clamp(0., 255.) as u8);
    }
    let blurred = imageops::blur(&small, 4.);
    let large = imageops::resize(&blurred, SIDE, SIDE, FilterType::CatmullRom);
    Ok(imageops::blur(&large, 6.))
}

/// K-means over a thumbnail, ranked by population with a bonus for saturation
/// so a small vivid area can outrank a large gray one.
fn palette(image: &RgbImage) -> [u32; PALETTE_SIZE] {
    let thumb = imageops::resize(image, 24, 24, FilterType::Triangle);
    let mut pixels = thumb
        .pixels()
        .map(|pixel| pixel.0.map(f32::from))
        .collect::<Vec<_>>();
    pixels.sort_by(|a, b| luma(a).total_cmp(&luma(b)));
    let mut centers: [[f32; 3]; PALETTE_SIZE] =
        std::array::from_fn(|i| pixels[(2 * i + 1) * pixels.len() / (2 * PALETTE_SIZE)]);
    let mut counts = [0usize; PALETTE_SIZE];
    for _ in 0..8 {
        let mut sums = [[0f32; 3]; PALETTE_SIZE];
        counts = [0; PALETTE_SIZE];
        for pixel in &pixels {
            let nearest = (0..PALETTE_SIZE)
                .min_by(|&a, &b| {
                    distance(pixel, &centers[a]).total_cmp(&distance(pixel, &centers[b]))
                })
                .unwrap_or(0);
            counts[nearest] += 1;
            for channel in 0..3 {
                sums[nearest][channel] += pixel[channel];
            }
        }
        for (center, (sum, count)) in centers.iter_mut().zip(sums.iter().zip(counts)) {
            if count > 0 {
                *center = sum.map(|channel| channel / count as f32);
            }
        }
    }
    let mut ranked = (0..PALETTE_SIZE).collect::<Vec<_>>();
    let score = |i: usize| counts[i] as f32 * (0.35 + saturation(&centers[i]));
    ranked.sort_by(|&a, &b| score(b).total_cmp(&score(a)));
    std::array::from_fn(|i| {
        let [r, g, b] = centers[ranked[i]].map(|channel| channel.round() as u32);
        (r << 16) | (g << 8) | b
    })
}

fn luma(pixel: &[f32; 3]) -> f32 {
    0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]
}

fn saturation(pixel: &[f32; 3]) -> f32 {
    let max = pixel.iter().copied().fold(0., f32::max);
    let min = pixel.iter().copied().fold(255., f32::min);
    if max <= 0. { 0. } else { (max - min) / max }
}

fn distance(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}
