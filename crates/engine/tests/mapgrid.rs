//! MAPGRID and NORTHARROW: coordinate grid, frame, labels, north arrow.

use cadcraft_doc::EntityKind;
use cadcraft_engine::{Input, Session};
use serde_json::json;

fn count(s: &Session, f: impl Fn(&EntityKind) -> bool) -> usize {
    s.doc().unwrap().model.iter().filter(|e| f(&e.kind)).count()
}

fn texts(s: &Session) -> Vec<String> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect()
}

#[test]
fn full_lines_frame_and_labels_for_an_extent() {
    let mut s = Session::new();
    let r = s.execute("mapgrid", &json!({ "extent": [499_995.0, 1_519_995.0, 500_105.0, 1_520_075.0], "spacing": 50 })).unwrap();
    // Eastings 500000, 500050, 500100; northings 1520000, 1520050.
    assert_eq!((r["columns"].as_u64(), r["rows"].as_u64()), (Some(3), Some(2)));
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Line(_))), 5);
    assert_eq!(count(&s, |k| matches!(k, EntityKind::LwPolyline(_))), 2, "inner and outer frame");
    let t = texts(&s);
    // The default text height is 50 / 12: the labels next to the corners would run into the
    // other side's labels, so only the ones that fit are drawn.
    assert!(t.contains(&"E 500050".to_string()) && t.contains(&"N 1520050".to_string()), "{t:?}");
    assert!(!t.contains(&"E 500000".to_string()), "{t:?}");
    s.undo().unwrap();
    assert_eq!(count(&s, |_| true), 0, "one undo removes the whole grid");
}

#[test]
fn crosses_are_short_and_the_frame_can_be_left_out() {
    let mut s = Session::new();
    s.execute(
        "mapgrid",
        &json!({ "extent": [0, 0, 100, 100], "spacing": 50, "style": "cross", "crossSize": 2, "frame": false, "labelPrefix": false }),
    )
    .unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::LwPolyline(_))), 0);
    let longest = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some((l.b.x - l.a.x).hypot(l.b.y - l.a.y)) } else { None })
        .fold(0.0f64, f64::max);
    assert!(longest <= 4.0 + 1e-9, "{longest}");
    assert!(texts(&s).contains(&"50".to_string()));
}

#[test]
fn the_grid_follows_an_attached_image() {
    let dir = std::env::temp_dir().join(format!("cc-mapgrid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("o.png");
    image::RgbaImage::new(100, 50).save(&png).unwrap();
    std::fs::write(dir.join("o.pgw"), "1\n0\n0\n-1\n1000.5\n2049.5\n").unwrap();
    let mut s = Session::new();
    s.execute("imageattach", &json!({ "path": png.to_string_lossy() })).unwrap();
    let r = s.execute("mapgrid", &json!({ "spacing": 20 })).unwrap();
    assert_eq!(r["extent"], json!([1000.0, 2000.0, 1100.0, 2050.0]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_prompts_ask_spacing_style_and_text_height() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [100, 60] })).unwrap();
    s.start("mapgrid").unwrap();
    s.input(Input::Enter).unwrap(); // no object picked: the whole drawing
    s.input(Input::Text("25".into())).unwrap();
    s.input(Input::Text("cross".into())).unwrap();
    s.input(Input::Text("1.5".into())).unwrap();
    assert!(s.running.is_none());
    assert!(!texts(&s).is_empty());
    assert!(texts(&s).iter().any(|t| t == "E 25"));
}

#[test]
fn north_arrow_is_drawn_and_turns() {
    let mut s = Session::new();
    let r = s.execute("northarrow", &json!({ "at": [10, 10], "size": 8, "rotation": 90 })).unwrap();
    assert_eq!(r["rotation"], json!(90.0));
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Solid(_))), 1);
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 1);
    assert_eq!(texts(&s), vec!["N".to_string()]);
}

#[test]
fn bad_input_is_an_error_and_changes_nothing() {
    let mut s = Session::new();
    for p in [
        json!({}),
        json!({ "extent": [0, 0, 0, 10] }),
        json!({ "extent": [0, 0, 1, 1], "spacing": 0 }),
        json!({ "extent": [0, 0, 1, 1], "spacing": -5 }),
        json!({ "extent": [0, 0, 1e9, 1e9], "spacing": 1 }),
        json!({ "extent": [0, 0, 100, 100], "style": "dots" }),
        json!({ "extent": [0, 0, 100, 100], "decimals": 99 }),
        json!({ "extent": [0, 0, 1e30, 1e30] }),
        json!({ "extent": "wide" }),
        json!({ "extent": [0, 0, 100, 100], "layerPrefix": "a/b" }),
    ] {
        assert!(s.execute("mapgrid", &p).is_err(), "{p}");
    }
    assert!(s.execute("northarrow", &json!({ "at": [0, 0], "size": -1 })).is_err());
    assert_eq!(count(&s, |_| true), 0);
}

#[test]
fn the_grid_can_frame_a_picked_rectangle_or_image() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [1000, 2000], "p2": [1100, 2060] })).unwrap();
    s.execute("circle", &json!({ "center": [5000, 5000], "radius": 10 })).unwrap();
    let rect = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::LwPolyline(_))).map(|e| e.handle).unwrap();
    // Programmatic: `handles` frames only the rectangle, not the far-away circle.
    let r = s.execute("mapgrid", &json!({ "handles": [rect.hex()], "spacing": 50 })).unwrap();
    assert_eq!(r["extent"], json!([1000.0, 2000.0, 1100.0, 2060.0]));
    // Prompted: the pick frames the grid.
    s.undo().unwrap();
    s.start("mapgrid").unwrap();
    s.input(Input::Pick(vec![rect])).unwrap();
    s.input(Input::Text("20".into())).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_none());
    assert!(texts(&s).iter().any(|t| t == "E 1020"), "{:?}", texts(&s));
    assert!(!texts(&s).iter().any(|t| t.contains("5000")), "the circle stays out of the grid");
}

#[test]
fn a_selected_object_frames_the_grid_at_once() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [100, 60] })).unwrap();
    s.execute("circle", &json!({ "center": [900, 900], "radius": 10 })).unwrap();
    let rect = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::LwPolyline(_))).map(|e| e.handle).unwrap();
    s.set_selection(vec![rect]);
    s.start("mapgrid").unwrap();
    assert!(s.prompt_text().contains("Grid spacing"), "{}", s.prompt_text());
}

#[test]
fn grid_and_arrow_follow_their_layers_colours() {
    use cadcraft_color::Color;
    let mut s = Session::new();
    s.execute("mapgrid", &json!({ "extent": [0, 0, 100, 100], "spacing": 50 })).unwrap();
    s.execute("northarrow", &json!({ "at": [50, 50], "size": 10 })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.model.iter().all(|e| e.common.color == Color::ByLayer), "recolouring a layer recolours its objects");
    for (name, idx) in [("GRID-LINES", 8), ("GRID-FRAME", 7), ("GRID-TEXT", 3), ("NORTH", 7)] {
        let l = d.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("layer {name}"));
        assert_eq!(l.color, Color::Index(idx), "{name}");
    }
}
