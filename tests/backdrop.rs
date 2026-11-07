use image::{Rgb, RgbImage};
use sonora::backdrop::{backdrop_path, load_or_render};

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
