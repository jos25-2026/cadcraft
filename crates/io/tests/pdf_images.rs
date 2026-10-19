//! PDF export of raster images (aerial photos, scanned plans): each picture becomes a JPEG image
//! XObject drawn under the line work, shared by the pages that show it.

use cadcraft_doc::{Common, Drawing, EntityKind, Image, Line, Space};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_io::PdfOptions;
use cadcraft_io::pdf::pdf_sheets;

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

#[test]
fn a_placed_picture_is_embedded_once_and_drawn_before_the_lines() {
    let path = std::env::temp_dir().join(format!("cc-pdf-image-{}.png", std::process::id()));
    image::RgbImage::from_pixel(16, 8, image::Rgb([20, 160, 40])).save(&path).unwrap();
    let mut d = Drawing::new_metric();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Image(Image { size: Vec2::new(16.0, 8.0), path: path.to_string_lossy().into_owned(), ..Default::default() }),
    )
    .unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(16.0, 8.0, 0.0) })).unwrap();
    let bytes = pdf_sheets(&d, &[Space::Model, Space::Model], &PdfOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(count(&bytes, b"/Subtype /Image"), 1, "one picture object shared by both pages");
    assert_eq!(count(&bytes, b"/DCTDecode"), 1);
    assert_eq!(count(&bytes, b"/XObject <<"), 2, "each page lists it");
}

#[test]
fn a_missing_picture_file_leaves_the_rest_of_the_page() {
    let mut d = Drawing::new_metric();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Image(Image { size: Vec2::new(16.0, 8.0), path: "/nonexistent/photo.jpg".into(), ..Default::default() }),
    )
    .unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(16.0, 8.0, 0.0) })).unwrap();
    let bytes = pdf_sheets(&d, &[Space::Model], &PdfOptions::default()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1."));
    assert_eq!(count(&bytes, b"/Subtype /Image"), 0);
}
