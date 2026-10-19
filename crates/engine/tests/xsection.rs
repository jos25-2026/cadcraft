//! XSECTION: cross sections of a sloping surface along a centerline.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

/// A 100 x 100 grid of points on the plane z = 0.1 x + 0.02 y.
fn plane() -> Session {
    let mut text = String::new();
    let mut n = 0;
    for ix in 0..=10 {
        for iy in 0..=10 {
            n += 1;
            let (x, y) = (f64::from(ix) * 10.0, f64::from(iy) * 10.0);
            text.push_str(&format!("{n},{y},{x},{}\n", 0.1 * x + 0.02 * y));
        }
    }
    let mut s = Session::new();
    s.execute("importpoints", &json!({ "text": text, "format": "PNEZ" })).unwrap();
    s
}

fn layer_count(s: &Session, layer: &str) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.common.layer == layer).count()
}

#[test]
fn sections_follow_the_surface_across_the_centerline() {
    let mut s = plane();
    let r = s.execute("xsection", &json!({ "points": [[10, 50], [90, 50]], "interval": 20, "width": 10 })).unwrap();
    assert_eq!(r["sections"], 5, "stations 0, 20, 40, 60, 80");
    // The first section crosses x = 10 from y = 40 to 60, the last x = 90.
    let (zmin, zmax) = (r["zMin"].as_f64().unwrap(), r["zMax"].as_f64().unwrap());
    assert!((zmin - 1.8).abs() < 1e-6 && (zmax - 10.2).abs() < 1e-6, "{zmin} {zmax}");
    assert_eq!(layer_count(&s, "XS-GROUND"), 5, "one ground profile per section");
    assert_eq!(layer_count(&s, "XS-LINES"), 5, "a section line per station on the plan");
    // Every profile vertex sits inside its cell: no profile leaves the sheet.
    let [cw, ch] = [r["cell"][0].as_f64().unwrap(), r["cell"][1].as_f64().unwrap()];
    assert!(cw > 20.0 && ch > 0.0);
    let d = s.doc().unwrap();
    let profile = d.model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if e.common.layer == "XS-GROUND" => Some(p.vertices.len()),
        _ => None,
    });
    assert_eq!(profile, Some(101), "a hundred steps across");
}

#[test]
fn a_drawn_polyline_can_be_the_centerline_and_sections_can_be_one_sided() {
    let mut s = plane();
    s.execute("line", &json!({ "points": [[20, 20], [80, 80]] })).unwrap();
    let h = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle.hex()).unwrap();
    let r =
        s.execute("xsection", &json!({ "centerline": h, "interval": 40, "leftWidth": 0.5, "rightWidth": 15, "vScale": 2, "columns": 2 })).unwrap();
    assert!(r["sections"].as_u64().unwrap() >= 3, "{r}");
    assert_eq!(r["vScale"], 2.0);
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let mut s = plane();
    let before = s.doc().unwrap().model.iter().count();
    for p in [
        json!({}),
        json!({ "points": [[1, 1]] }),
        json!({ "points": [[1, 1], [1, 1]] }),
        json!({ "points": [[1, 1], ["a", 2]] }),
        json!({ "points": [[1, 1], [1e300, 2]] }),
        json!({ "centerline": "ZZZZ" }),
        json!({ "points": [[10, 50], [90, 50]], "interval": -1 }),
        json!({ "points": [[10, 50], [90, 50]], "interval": 1e-9 }),
        json!({ "points": [[10, 50], [90, 50]], "width": 0 }),
        json!({ "points": [[10, 50], [90, 50]], "step": 1e-9 }),
        json!({ "points": [[10, 50], [90, 50]], "columns": 0 }),
        json!({ "points": [[10, 50], [90, 50]], "vScale": "x" }),
        json!({ "points": [[10, 50], [90, 50]], "layerPrefix": "a<b" }),
        json!({ "points": [[1000, 1000], [2000, 1000]] }),
    ] {
        assert!(s.execute("xsection", &p).is_err(), "{p}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), before);
    assert!(Session::new().execute("xsection", &json!({ "points": [[0, 0], [10, 0]] })).is_err(), "no surface at all");
}

#[test]
fn centerlines_are_named_and_sections_cut_along_them_with_horizontal_lines() {
    let mut s = plane();
    let r = s.execute("alignment", &json!({ "points": [[10, 50], [90, 50]] })).unwrap();
    assert_eq!(r["name"], "Line 1A");
    assert_eq!(s.execute("alignment", &json!({ "points": [[10, 20], [90, 20]] })).unwrap()["name"], "Line 2A");
    assert_eq!(s.execute("alignment", &json!({ "points": [[10, 30], [90, 30]], "name": "Road" })).unwrap()["layer"], "CL-Road");
    assert!(s.execute("alignment", &json!({ "points": [[1, 1], [5, 5]], "name": "road" })).is_err(), "a name is used once");
    assert!(s.execute("alignment", &json!({ "points": [[1, 1]] })).is_err());
    assert!(s.execute("alignment", &json!({ "points": [[1, 1], [5, 5]], "name": "a<b" })).is_err());

    let r = s.execute("xsection", &json!({ "name": "Line 1A", "interval": 20, "width": 10, "gridEvery": 1 })).unwrap();
    assert_eq!(r["sections"], 5);
    // The sections span about 1.8 to 10.2 above a datum just under that: lines every 1.
    let (zmin, zmax) = (r["zMin"].as_f64().unwrap(), r["zMax"].as_f64().unwrap());
    let datum = r["datum"].as_f64().unwrap();
    assert!(datum <= zmin && zmax - datum >= 8.0);
    let per_section = layer_count(&s, "XS-GRID") / 5;
    assert_eq!(per_section as f64, ((zmax - datum) / 1.0).floor());
    assert!(s.execute("xsection", &json!({ "name": "Nope" })).is_err());
    assert!(s.execute("xsection", &json!({ "name": "Line 1A", "gridEvery": -1 })).is_err());
    assert!(s.execute("xsection", &json!({ "name": "Line 1A", "gridEvery": 1e-9 })).is_err(), "too many lines");
    // Without gridEvery there are no horizontal lines.
    let mut s2 = plane();
    s2.execute("alignment", &json!({ "points": [[10, 50], [90, 50]] })).unwrap();
    s2.execute("xsection", &json!({ "name": "Line 1A", "interval": 40 })).unwrap();
    assert_eq!(layer_count(&s2, "XS-GRID"), 0);
}

#[test]
fn vertical_lines_mark_the_distance_across() {
    let mut s = plane();
    s.execute("alignment", &json!({ "points": [[10, 50], [90, 50]] })).unwrap();
    s.execute("xsection", &json!({ "name": "Line 1A", "interval": 40, "width": 10, "vGridEvery": 5 })).unwrap();
    // Three sections; offsets -10, -5, 0, 5, 10: four lines (the centerline has its own) and five labels.
    assert_eq!(layer_count(&s, "XS-GRID"), 3 * 4);
    let labels: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) if e.common.layer == "XS-TEXT" && t.value.chars().all(|c| c.is_ascii_digit()) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels.iter().filter(|l| l.as_str() == "10").count(), 6, "{labels:?}");
    assert!(s.execute("xsection", &json!({ "name": "Line 1A", "vGridEvery": 1e-9 })).is_err());
    assert!(s.execute("xsection", &json!({ "name": "Line 1A", "vGridEvery": -2 })).is_err());
}
