use anyhow::{Context, Result};
use image::{ImageReader, RgbImage, imageops, imageops::FilterType};
use std::path::{Path, PathBuf};

const SIDE: u32 = 192;
const PALETTE_SIZE: usize = 3;

/// The share of the backdrop's width behind Now Playing's details column;
/// lyrics fill the rest.
const DETAILS_WIDTH: f32 = 0.38;
/// The middle band of the square backdrop that a wide window shows.
const VISIBLE_BAND: f32 = 0.6;
/// WCAG AA contrast for body text.
pub const READABLE: f32 = 4.5;
/// The most extra scrim a column may get to keep its text readable.
const MAX_PLATE: f32 = 0.7;

/// A pre-blurred copy of album art with its prominent colors.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub path: PathBuf,
    /// 0xRRGGBB colors, most prominent first.
    pub palette: [u32; PALETTE_SIZE],
    /// Brightness behind the details column, then behind the lyrics.
    pub tones: [Tone; 2],
}

/// The darkest and brightest tenth of a region, as sRGB luma (0–255).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub low: f32,
    pub high: f32,
}

/// A text color and the background it belongs with, as sRGB luma (0–255).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub text: f32,
    pub background: f32,
}

/// How Now Playing keeps its text readable over a backdrop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Legibility {
    /// Light text (the dark palette) rather than dark text.
    pub light_text: bool,
    /// The background luma the chosen text must stand out from in the
    /// worst spot of each column, after any plate.
    pub worst: [f32; 2],
    /// Extra opacity of the text palette's background laid behind each
    /// column; zero unless the scrim alone leaves text unreadable.
    pub plates: [f32; 2],
}

/// WCAG contrast ratio between two sRGB luma values (0–255).
pub fn contrast(a: f32, b: f32) -> f32 {
    let linear = |luma: f32| {
        let c = (luma / 255.).clamp(0., 1.);
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (a, b) = (linear(a), linear(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Chooses light or dark text for a backdrop seen through a scrim of luma
/// `scrim` at opacity `alpha`, whichever reads better in each column's worst
/// spot. Columns that still fall short of [`READABLE`] get a plate: just
/// enough of that text's own background to reach it.
pub fn legibility(tones: [Tone; 2], scrim: f32, alpha: f32, light: Ink, dark: Ink) -> Legibility {
    let seen = |luma: f32| alpha * scrim + (1. - alpha) * luma;
    let worst_for = |ink: &Ink, tone: &Tone| {
        // Light text struggles on the brightest spot; dark text on the darkest.
        let spot = if ink.text > ink.background {
            seen(tone.high)
        } else {
            seen(tone.low)
        };
        (spot, contrast(ink.text, spot))
    };
    let score = |ink: &Ink| {
        tones
            .iter()
            .map(|tone| worst_for(ink, tone).1)
            .fold(f32::INFINITY, f32::min)
    };
    let light_text = score(&light) >= score(&dark);
    let ink = if light_text { light } else { dark };
    let mut worst = [0.; 2];
    let mut plates = [0.; 2];
    for (index, tone) in tones.iter().enumerate() {
        let (spot, ratio) = worst_for(&ink, tone);
        let mut plate = 0.;
        let mut under = spot;
        if ratio < READABLE {
            while plate < MAX_PLATE {
                plate = (plate + 0.05_f32).min(MAX_PLATE);
                under = plate * ink.background + (1. - plate) * spot;
                if contrast(ink.text, under) >= READABLE {
                    break;
                }
            }
        }
        worst[index] = under;
        plates[index] = plate;
    }
    Legibility {
        light_text,
        worst,
        plates,
    }
}

/// The brightness range behind the details column and behind the lyrics.
pub fn tones(image: &RgbImage) -> [Tone; 2] {
    let (width, height) = image.dimensions();
    let split = (width as f32 * DETAILS_WIDTH).round() as u32;
    let top = (height as f32 * (1. - VISIBLE_BAND) / 2.).round() as u32;
    let bottom = height - top;
    let tone = |columns: std::ops::Range<u32>| {
        let mut lumas = columns
            .flat_map(|x| (top..bottom).map(move |y| (x, y)))
            .map(|(x, y)| luma(&image.get_pixel(x, y).0.map(f32::from)))
            .collect::<Vec<_>>();
        if lumas.is_empty() {
            return Tone { low: 0., high: 0. };
        }
        lumas.sort_by(f32::total_cmp);
        let at = |share: f32| lumas[((lumas.len() - 1) as f32 * share).round() as usize];
        Tone {
            low: at(0.1),
            high: at(0.9),
        }
    };
    [tone(0..split), tone(split..width)]
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
        tones: tones(&image),
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
