//! TIN and CONTOUR: surface from survey points, contour lines through it.

use cadcraft_doc::EntityKind;
use cadcraft_engine::{Input, Session};
use serde_json::json;

/// A survey of a 100 × 100 m hill: z = 50 - distance from the centre / 2, on a 10 m grid.
fn hill(s: &mut Session) {
    let mut csv = String::new();
    let mut n = 1;
    for i in 0..=10 {
        for j in 0..=10 {
            let (e, nn) = (500_000.0 + f64::from(i) * 10.0, 1_520_000.0 + f64::from(j) * 10.0);
            let d = ((e - 500_050.0).powi(2) + (nn - 1_520_050.0).powi(2)).sqrt();
            csv.push_str(&format!("{n},{nn},{e},{:.3},GRD\n", 50.0 - d / 2.0));
            n += 1;
        }
    }
    s.execute("importpoints", &json!({ "text": csv, "showNumber": false, "showElevation": false, "showDescription": false })).unwrap();
}

fn count(s: &Session, f: impl Fn(&EntityKind) -> bool) -> usize {
    s.doc().unwrap().model.iter().filter(|e| f(&e.kind)).count()
}

#[test]
fn tin_triangulates_the_survey_points() {
    let mut s = Session::new();
    hill(&mut s);
    let r = s.execute("tin", &json!({})).unwrap();
    assert_eq!(r["points"], json!(121));
    // 10 × 10 cells, two triangles each.
    assert_eq!(r["triangles"], json!(200));
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Face3d(_))), 200);
    assert!((r["zMax"].as_f64().unwrap() - 50.0).abs() < 1e-9);
    s.undo().unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Face3d(_))), 0, "one undo removes the surface");
}

#[test]
fn max_edge_drops_the_long_hull_triangles() {
    let mut s = Session::new();
    for (x, y, z) in [(0.0, 0.0, 1.0), (10.0, 0.0, 2.0), (0.0, 10.0, 3.0), (100.0, 100.0, 4.0)] {
        s.execute("point", &json!({ "at": [x, y, z] })).unwrap();
    }
    let all = s.execute("tin", &json!({})).unwrap()["triangles"].as_u64().unwrap();
    s.undo().unwrap();
    let near = s.execute("tin", &json!({ "maxEdge": 20 })).unwrap()["triangles"].as_u64().unwrap();
    assert!(all > near && near == 1, "{all} {near}");
}

#[test]
fn contours_follow_the_hill() {
    let mut s = Session::new();
    hill(&mut s);
    let r = s.execute("contour", &json!({ "interval": 5, "majorEvery": 2 })).unwrap();
    // Levels 15..45 (the peak at 50 is a single point). The square survey cuts the lower rings
    // into arcs; the rings at 30, 35, 40 and 45 m (radius 40 m or less) stay whole.
    let mut rings = 0;
    for e in s.doc().unwrap().model.iter() {
        if let EntityKind::LwPolyline(p) = &e.kind {
            assert!(p.elevation >= 15.0 && p.elevation <= 45.0, "{}", p.elevation);
            if p.closed {
                rings += 1;
            }
        }
    }
    assert_eq!(rings, 4, "closed contours");
    assert!(r["labels"].as_u64().unwrap() > 0);
    let labels: Vec<String> =
        s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect();
    assert!(labels.iter().all(|l| l.parse::<f64>().map(|v| (v / 5.0).fract() == 0.0 && ((v / 5.0) as i64) % 2 == 0).unwrap_or(false)), "{labels:?}");
}

#[test]
fn contour_builds_a_tin_when_there_is_none() {
    let mut s = Session::new();
    hill(&mut s);
    let r = s.execute("contour", &json!({ "interval": 10 })).unwrap();
    assert_eq!(r["triangles"], json!(200));
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Face3d(_))), 0, "no TIN objects were added");
}

#[test]
fn prompts_ask_the_interval_and_major_spacing() {
    let mut s = Session::new();
    hill(&mut s);
    s.start("tin").unwrap();
    s.input(Input::Enter).unwrap(); // every point
    s.input(Input::Enter).unwrap(); // keep all triangles
    assert!(s.running.is_none());
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Face3d(_))), 200);
    s.start("contour").unwrap();
    s.input(Input::Text("10".into())).unwrap();
    s.input(Input::Text("3".into())).unwrap();
    assert!(s.running.is_none());
    assert!(count(&s, |k| matches!(k, EntityKind::LwPolyline(_))) >= 3);
}

#[test]
fn bad_input_is_an_error_and_changes_nothing() {
    let mut s = Session::new();
    assert!(s.execute("tin", &json!({})).is_err(), "no points");
    assert!(s.execute("contour", &json!({})).is_err(), "no surface");
    s.execute("point", &json!({ "at": [0, 0, 1] })).unwrap();
    s.execute("point", &json!({ "at": [1, 1, 2] })).unwrap();
    assert!(s.execute("tin", &json!({})).is_err(), "two points");
    // A flat survey has nothing to contour.
    for (x, y) in [(10.0, 0.0), (0.0, 10.0)] {
        s.execute("point", &json!({ "at": [x, y, 5.0] })).unwrap();
    }
    hill(&mut s);
    for p in [
        json!({ "interval": 0 }),
        json!({ "interval": -1 }),
        json!({ "majorEvery": 0 }),
        json!({ "decimals": 99 }),
        json!({ "layerPrefix": "a/b" }),
        json!({ "interval": 1e-9 }),
    ] {
        assert!(s.execute("contour", &p).is_err(), "{p}");
    }
    assert!(s.execute("tin", &json!({ "maxEdge": -1 })).is_err());
    assert!(s.execute("tin", &json!({ "layer": "x:y" })).is_err());
}
