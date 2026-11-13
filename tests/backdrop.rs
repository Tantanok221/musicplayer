use image::{Rgb, RgbImage};
use sonora::backdrop::{
    Ink, READABLE, Tone, backdrop_path, contrast, legibility, load_or_render, tones,
};

fn channels(color: u32) -> [u32; 3] {
    [color >> 16, (color >> 8) & 0xff, color & 0xff]
}

#[test]
fn backdrop_blurs_art_once_and_finds_its_colors() {
    let dir = tempfile::tempdir().unwrap();
    let art = dir.path().join("cover.art");
    let image = RgbImage::from_fn(300, 300, |x, _| {
        if x < 200 {
            Rgb([200, 30, 40])
        } else {
            Rgb([30, 60, 210])
        }
    });
    image
        .save_with_format(&art, image::ImageFormat::Jpeg)
        .unwrap();

    let backdrop = load_or_render(&art).unwrap();
    assert_eq!(backdrop.path, backdrop_path(&art));
    assert!(backdrop.path.exists());
    let [r, g, b] = channels(backdrop.palette[0]);
    assert!(r > g + 60 && r > b + 60, "red should lead: {r} {g} {b}");
    assert!(
        backdrop.palette.iter().any(|&color| {
            let [r, _, b] = channels(color);
            b > r + 60
        }),
        "blue should appear: {:06x?}",
        backdrop.palette
    );

    std::fs::remove_file(&art).unwrap();
    assert_eq!(load_or_render(&art).unwrap(), backdrop);
}

/// Sonora's own palettes as luma: light text on the dark theme's background,
/// and dark text on the light theme's.
const LIGHT_TEXT: Ink = Ink {
    text: 218.,
    background: 28.,
};
const DARK_TEXT: Ink = Ink {
    text: 40.,
    background: 250.,
};
const DARK_SCRIM: (f32, f32) = (28., 0.55);
const LIGHT_SCRIM: (f32, f32) = (250., 0.62);

fn flat(luma: f32) -> [Tone; 2] {
    [Tone {
        low: luma,
        high: luma,
    }; 2]
}

fn readable(tones: [Tone; 2], (scrim, alpha): (f32, f32)) -> sonora::backdrop::Legibility {
    let choice = legibility(tones, scrim, alpha, LIGHT_TEXT, DARK_TEXT);
    let text = if choice.light_text {
        LIGHT_TEXT.text
    } else {
        DARK_TEXT.text
    };
    for worst in choice.worst {
        let ratio = contrast(text, worst);
        assert!(ratio >= READABLE, "{ratio:.2}:1 for {choice:?}");
    }
    choice
}

#[test]
fn tones_measure_the_details_and_lyrics_columns_separately() {
    let image = RgbImage::from_fn(100, 100, |x, _| {
        if x < 38 {
            Rgb([250, 250, 250])
        } else {
            Rgb([5, 5, 5])
        }
    });
    let [details, lyrics] = tones(&image);
    assert!(details.low > 240. && details.high > 240., "{details:?}");
    assert!(lyrics.low < 10. && lyrics.high < 10., "{lyrics:?}");
}

#[test]
fn white_art_in_dark_mode_gets_dark_text() {
    let choice = readable(flat(245.), DARK_SCRIM);
    assert!(!choice.light_text);
}

#[test]
fn black_art_in_light_mode_keeps_dark_text_readable() {
    // The light theme's stronger scrim lifts black art enough for dark text;
    // only its muted shades need adjusting, which the interface does.
    let choice = readable(flat(10.), LIGHT_SCRIM);
    assert!(!choice.light_text);
    assert_eq!(choice.plates, [0., 0.]);
}

#[test]
fn art_that_already_suits_the_theme_keeps_its_text_and_no_plate() {
    let dark = readable(flat(20.), DARK_SCRIM);
    assert!(dark.light_text);
    assert_eq!(dark.plates, [0., 0.]);
    let light = readable(flat(240.), LIGHT_SCRIM);
    assert!(!light.light_text);
    assert_eq!(light.plates, [0., 0.]);
}

#[test]
fn high_contrast_art_gets_a_plate_only_where_needed() {
    // Bright details beside dark lyrics: no single text color reads on both.
    let tones = [
        Tone {
            low: 200.,
            high: 250.,
        },
        Tone { low: 5., high: 40. },
    ];
    let choice = readable(tones, DARK_SCRIM);
    assert!(choice.plates.iter().any(|&plate| plate > 0.), "{choice:?}");
    assert!(choice.plates.contains(&0.), "{choice:?}");
    // The light theme's scrim is strong enough for dark text on both.
    assert_eq!(readable(tones, LIGHT_SCRIM).plates, [0., 0.]);
}
