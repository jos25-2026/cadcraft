//! IMAGEATTACH: an image and its world file land where the survey says; refusals change nothing.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadcraft-imageattach-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(d: &std::path::Path, name: &str, w: u32, h: u32) -> String {
    let p = d.join(name);
    image::RgbaImage::new(w, h).save(&p).unwrap();
    p.to_string_lossy().into_owned()
}

fn only_image(s: &Session) -> cadcraft_doc::Image {
    let d = s.doc().unwrap();
    let mut it = d.model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None });
    let i = it.next().expect("one image");
    assert!(it.next().is_none());
    i
}

#[test]
fn a_world_file_gives_the_position_and_scale() {
    let d = dir("world");
    let img = png(&d, "site.png", 100, 50);
    // 0.5 m pixels, north up; the top-left pixel centre is at E 1000.25, N 2000.75.
    std::fs::write(d.join("site.pgw"), "0.5\n0\n0\n-0.5\n1000.25\n2000.75\n").unwrap();
    let mut s = Session::new();
    let r = s.execute("imageattach", &json!({ "path": img })).unwrap();
    assert_eq!(r["size"], json!([100, 50]));
    assert!(r["worldFile"].as_str().unwrap().ends_with("site.pgw"));
    let i = only_image(&s);
    assert!((i.insert.x - 1000.0).abs() < 1e-9 && (i.insert.y - 1976.0).abs() < 1e-9, "{:?}", i.insert);
    assert_eq!((i.u.x, i.u.y, i.v.x, i.v.y), (0.5, 0.0, 0.0, 0.5));
    // The far corner is 50 m east and 25 m north of the lower-left corner.
    let c = r["corners"].as_array().unwrap();
    assert_eq!(c[2], json!([1050.0, 2001.0]));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn explicit_insert_scale_and_rotation_override_the_world_file() {
    let d = dir("explicit");
    let img = png(&d, "plan.png", 10, 10);
    std::fs::write(d.join("plan.pgw"), "9\n0\n0\n-9\n1\n2\n").unwrap();
    let mut s = Session::new();
    s.execute("imageattach", &json!({ "path": img, "insert": [100, 200], "scale": 2, "rotation": 90 })).unwrap();
    let i = only_image(&s);
    assert_eq!((i.insert.x, i.insert.y), (100.0, 200.0));
    assert!(i.u.x.abs() < 1e-12 && (i.u.y - 2.0).abs() < 1e-12, "u points up after 90 degrees: {:?}", i.u);
    assert!((i.v.x + 2.0).abs() < 1e-12 && i.v.y.abs() < 1e-12, "v points left: {:?}", i.v);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn world_file_false_ignores_the_file_beside_the_image() {
    let d = dir("nofile");
    let img = png(&d, "a.png", 4, 4);
    std::fs::write(d.join("a.pgw"), "3\n0\n0\n-3\n50\n60\n").unwrap();
    let mut s = Session::new();
    let r = s.execute("imageattach", &json!({ "path": img, "worldFile": false })).unwrap();
    assert!(r["worldFile"].is_null());
    assert_eq!(only_image(&s).u.x, 1.0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn bad_input_is_an_error_and_changes_nothing() {
    let d = dir("bad");
    let good = png(&d, "ok.png", 4, 4);
    let junk = d.join("junk.png");
    std::fs::write(&junk, b"not an image").unwrap();
    let broken = png(&d, "broken.png", 4, 4);
    std::fs::write(d.join("broken.pgw"), "1\n0\n0\n").unwrap();
    let mut s = Session::new();
    let n = s.doc().unwrap().model.iter().count();
    for params in [
        json!({}),
        json!({ "path": "/nonexistent/x.png" }),
        json!({ "path": junk.to_string_lossy() }),
        json!({ "path": broken }),
        json!({ "path": good, "scale": 0 }),
        json!({ "path": good, "insert": [1] }),
        json!({ "path": good, "worldFile": "/nonexistent/x.pgw" }),
        json!({ "path": good, "worldFile": 5 }),
    ] {
        assert!(s.execute("imageattach", &params).is_err(), "{params}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), n);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn prompt_asks_for_the_path_and_places_the_image() {
    let d = dir("prompt");
    let img = png(&d, "typed.png", 8, 6);
    let mut s = Session::new();
    s.start("imageattach").unwrap();
    // A path pasted from Explorer arrives in quotes.
    s.input(cadcraft_engine::Input::Text(format!("\"{img}\""))).unwrap();
    assert_eq!(only_image(&s).size.x, 8.0);
    let _ = std::fs::remove_dir_all(&d);
}
