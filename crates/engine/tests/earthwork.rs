//! EARTHWORK: cut and fill volumes against the TIN.

use cadcraft_doc::EntityKind;
use cadcraft_engine::{Input, Session};
use serde_json::json;

/// A 100 × 100 m plane rising 10 m from west to east (z = x / 10), on a 10 m grid.
fn slope(s: &mut Session) {
    let mut csv = String::new();
    let mut n = 1;
    for i in 0..=10 {
        for j in 0..=10 {
            let (e, nn) = (f64::from(i) * 10.0, f64::from(j) * 10.0);
            csv.push_str(&format!("{n},{nn},{e},{:.3},GRD\n", e / 10.0));
            n += 1;
        }
    }
    s.execute("importpoints", &json!({ "text": csv, "showNumber": false, "showElevation": false, "showDescription": false })).unwrap();
}

#[test]
fn a_slope_balances_at_its_mean_height() {
    let mut s = Session::new();
    slope(&mut s);
    let r = s.execute("earthwork", &json!({ "label": false })).unwrap();
    assert!((r["elevation"].as_f64().unwrap() - 5.0).abs() < 1e-6, "{r}");
    assert!((r["cut"].as_f64().unwrap() - 12_500.0).abs() < 1e-3, "{r}");
    assert!((r["fill"].as_f64().unwrap() - 12_500.0).abs() < 1e-3, "{r}");
    assert!((r["area"].as_f64().unwrap() - 10_000.0).abs() < 1e-6);
    assert_eq!(r["balanced"], json!(true));
}

#[test]
fn a_design_elevation_below_the_ground_is_all_cut() {
    let mut s = Session::new();
    slope(&mut s);
    let r = s.execute("earthwork", &json!({ "elevation": 0, "label": false })).unwrap();
    assert!((r["cut"].as_f64().unwrap() - 50_000.0).abs() < 1e-3, "{r}");
    assert!(r["fill"].as_f64().unwrap().abs() < 1e-9);
}

#[test]
fn a_rectangle_limits_the_area() {
    let mut s = Session::new();
    slope(&mut s);
    // The eastern half only: x from 50 to 100, ground 5 to 10 m, design 5 m → cut 50 × 100 × 2.5.
    s.execute("rectang", &json!({ "p1": [50, 0], "p2": [100, 100] })).unwrap();
    let h = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::LwPolyline(_))).map(|e| e.handle.hex()).unwrap();
    let r = s.execute("earthwork", &json!({ "boundary": h, "elevation": 5, "label": false })).unwrap();
    assert!((r["area"].as_f64().unwrap() - 5_000.0).abs() < 1e-6, "{r}");
    assert!((r["cut"].as_f64().unwrap() - 12_500.0).abs() < 1e-3, "{r}");
    assert!(r["fill"].as_f64().unwrap().abs() < 1e-6);
}

#[test]
fn the_result_is_written_into_the_drawing() {
    let mut s = Session::new();
    slope(&mut s);
    let before = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
    s.execute("earthwork", &json!({ "elevation": 5 })).unwrap();
    let after = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
    assert_eq!(after - before, 5);
    s.undo().unwrap();
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let mut s = Session::new();
    assert!(s.execute("earthwork", &json!({})).is_err(), "no surface");
    slope(&mut s);
    for p in [json!({ "elevation": "x" }), json!({ "elevation": 1e300 }), json!({ "boundary": "zz" }), json!({ "boundary": "ffff" })] {
        assert!(s.execute("earthwork", &p).is_err(), "{p}");
    }
}

#[test]
fn the_prompts_ask_for_boundary_then_elevation() {
    let mut s = Session::new();
    slope(&mut s);
    let before = s.doc().unwrap().model.iter().count();
    s.start("earthwork").unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Text("5".into())).unwrap();
    assert_eq!(s.doc().unwrap().model.iter().count() - before, 5, "the five result lines");
}
