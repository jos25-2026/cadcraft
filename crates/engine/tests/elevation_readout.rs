//! The coordinate readout's Z: the elevation of the survey point, contour or TIN face under the cursor.

use cadcraft_engine::Session;
use cadcraft_geom::Vec2;
use serde_json::json;

#[test]
fn z_comes_from_the_point_the_contour_and_the_tin_under_the_cursor() {
    let mut s = Session::new();
    // Four corner points of a 10 × 10 square on the plane z = 100 + x, and a TIN between them.
    let file = std::env::temp_dir().join(format!("cc-elev-{}.xyz", std::process::id()));
    std::fs::write(&file, "0 0 100\n10 0 110\n10 10 110\n0 10 100\n").unwrap();
    s.execute("importpoints", &json!({ "path": file.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&file);
    // On a point: its own elevation.
    assert_eq!(s.elevation_at(Vec2::new(10.0, 10.0), 0.5), Some(110.0));
    // Nothing there: none.
    assert_eq!(s.elevation_at(Vec2::new(5.0, 5.0), 0.5), None);
    // Over a TIN triangle the surface elevation is interpolated.
    s.execute("tin", &json!({})).unwrap();
    let z = s.elevation_at(Vec2::new(5.0, 5.0), 0.01).unwrap();
    assert!((z - 105.0).abs() < 1e-9, "{z}");
    // A contour reads its own level.
    s.execute("contour", &json!({ "interval": 2 })).unwrap();
    let z = s.elevation_at(Vec2::new(4.0, 7.0), 0.05);
    assert_eq!(z, Some(104.0));
}

#[test]
fn hostile_cursor_values_give_none() {
    let s = Session::new();
    assert_eq!(s.elevation_at(Vec2::new(f64::NAN, 0.0), 1.0), None);
    assert_eq!(s.elevation_at(Vec2::new(0.0, 0.0), f64::NAN), None);
}
