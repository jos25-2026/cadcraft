//! Raster images (IMAGE entities) are drawn under the line work: the right pixels land in the
//! right place (the image's top row at the top), clipping and a missing file are handled.

use cadcraft_doc::{Common, Drawing, EntityKind, Image, Space};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_render::raster::{RasterOptions, View, render};
use cadcraft_render::{Options, build};

/// A 2 × 2 picture: red, green on top; blue, white below.
fn write_png(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("cadcraft-image-raster-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut img = image::RgbaImage::new(2, 2);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
    img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
    img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
    img.save(&path).unwrap();
    path.to_string_lossy().into_owned()
}

fn drawing_with(path: &str, tweak: impl FnOnce(&mut Image)) -> Drawing {
    let mut d = Drawing::new_imperial();
    // No frame, so only the picture is in the way.
    d.header.set_i64("IMAGEFRAME", 0);
    let mut im = Image {
        insert: Vec3::ZERO,
        u: Vec3::new(5.0, 0.0, 0.0),
        v: Vec3::new(0.0, 5.0, 0.0),
        size: Vec2::new(2.0, 2.0),
        path: path.to_string(),
        ..Image::default()
    };
    tweak(&mut im);
    d.add(&Space::Model, Common::default(), EntityKind::Image(im)).unwrap();
    d
}

/// 100 × 100 px view of the 10 × 10 unit area the picture covers.
fn draw(d: &Drawing) -> tiny_skia::Pixmap {
    let list = build(d, &Space::Model, &Options::default());
    let view = View { center: Vec2::new(5.0, 5.0), scale: 10.0, width: 100, height: 100 };
    render(&list, &view, &RasterOptions { antialias: false, ..Default::default() }).unwrap()
}

fn rgb(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
    let p = pm.pixel(x, y).unwrap();
    (p.red(), p.green(), p.blue())
}

/// The pixel is the colour to within the filter's rounding.
fn is(pm: &tiny_skia::Pixmap, x: u32, y: u32, want: (u8, u8, u8), what: &str) {
    let got = rgb(pm, x, y);
    let near = |a: u8, b: u8| a.abs_diff(b) <= 12;
    assert!(near(got.0, want.0) && near(got.1, want.1) && near(got.2, want.2), "{what}: got {got:?}, want {want:?}");
}

#[test]
fn pixels_land_in_their_quadrants_with_the_top_row_up() {
    let path = write_png("quad.png");
    let pm = draw(&drawing_with(&path, |_| {}));
    is(&pm, 25, 25, (255, 0, 0), "top-left");
    is(&pm, 75, 25, (0, 255, 0), "top-right");
    is(&pm, 25, 75, (0, 0, 255), "bottom-left");
    is(&pm, 75, 75, (255, 255, 255), "bottom-right");
}

#[test]
fn a_rectangular_clip_hides_the_rest() {
    let path = write_png("clip.png");
    // Pixel-space corners (origin top-left, y down): keep only the left column.
    let pm = draw(&drawing_with(&path, |i| {
        i.clipping = true;
        i.clip = vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 2.0)];
    }));
    is(&pm, 25, 25, (255, 0, 0), "left column");
    is(&pm, 25, 75, (0, 0, 255), "left column");
    let bg = RasterOptions::default().background;
    is(&pm, 75, 25, (bg.0, bg.1, bg.2), "clipped away");
    is(&pm, 75, 75, (bg.0, bg.1, bg.2), "clipped away");
}

#[test]
fn a_missing_file_draws_a_crossed_frame_and_no_panic() {
    let pm = draw(&drawing_with("/nonexistent/photo.png", |_| {}));
    let bg = RasterOptions::default().background;
    // The cross passes through the middle; away from it there is only background.
    assert_eq!(rgb(&pm, 75, 25), (bg.0, bg.1, bg.2));
    assert_ne!(rgb(&pm, 50, 50), (bg.0, bg.1, bg.2), "diagonals cross at the centre");
}

#[test]
fn an_unreadable_file_is_skipped() {
    let dir = std::env::temp_dir().join(format!("cadcraft-image-raster-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.png");
    std::fs::write(&junk, b"not a png").unwrap();
    let pm = draw(&drawing_with(&junk.to_string_lossy(), |_| {}));
    let bg = RasterOptions::default().background;
    assert_eq!(rgb(&pm, 25, 25), (bg.0, bg.1, bg.2));
}
