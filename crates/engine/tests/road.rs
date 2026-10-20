//! ROADALIGN: tangent-arc-tangent alignments with stations, and sections cut along them.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn layer_count(s: &Session, layer: &str) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.common.layer == layer).count()
}

#[test]
fn a_right_angle_curve_has_the_textbook_elements() {
    let mut s = Session::new();
    let r = s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0], [100, 100]], "radius": 20, "labels": false })).unwrap();
    assert_eq!(r["name"], "Line 1A");
    let c = &r["curves"][0];
    assert!(near(c["delta"].as_f64().unwrap(), 90.0, 1e-9));
    assert!(near(c["tangent"].as_f64().unwrap(), 20.0, 1e-9), "T = R tan(Δ/2)");
    assert!(near(c["length"].as_f64().unwrap(), 20.0 * std::f64::consts::FRAC_PI_2, 1e-9));
    assert!(near(c["external"].as_f64().unwrap(), 20.0 * (2f64.sqrt() - 1.0), 1e-9));
    assert!(near(c["tcStation"].as_f64().unwrap(), 80.0, 1e-9) && near(c["piStation"].as_f64().unwrap(), 100.0, 1e-9));
    assert!(near(r["length"].as_f64().unwrap(), 160.0 + 20.0 * std::f64::consts::FRAC_PI_2, 1e-9));
    assert_eq!(c["tc"], json!([80.0, 0.0]));
    // One polyline on the centerline layer, the arc as a bulge (a left turn is counter-clockwise).
    let pl = s.doc().unwrap().model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if e.common.layer == "CL-Line 1A" => Some(p.clone()),
        _ => None,
    });
    let pl = pl.unwrap();
    assert_eq!(pl.vertices.len(), 4);
    assert!(near(pl.vertices[1].bulge, (std::f64::consts::FRAC_PI_2 / 4.0).tan(), 1e-12));
    // The turn to the right gets a negative bulge, and a radius of zero leaves a corner.
    let r = s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0], [100, -100]], "radius": 20, "name": "R" })).unwrap();
    assert!(r["curves"][0]["delta"].as_f64().unwrap() > 89.0);
    s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0], [100, 100]], "radius": 0, "name": "Sharp" })).unwrap();
    let sharp = s.doc().unwrap().model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if e.common.layer == "CL-Sharp" => Some(p.vertices.len()),
        _ => None,
    });
    assert_eq!(sharp, Some(3));
    let right = s.doc().unwrap().model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if e.common.layer == "CL-R" => Some(p.vertices[1].bulge),
        _ => None,
    });
    assert!(right.unwrap() < 0.0);
}

#[test]
fn a_radius_can_be_given_per_pi_and_stations_can_be_marked() {
    let mut s = Session::new();
    let r = s
        .execute(
            "roadalign",
            &json!({ "pis": [[0, 0], [200, 0, 30], [200, 200, 60], [400, 200]], "ticks": 100, "startStation": 1000, "labels": true }),
        )
        .unwrap();
    assert_eq!(r["curves"].as_array().unwrap().len(), 2);
    assert_eq!(r["curves"][1]["radius"], 60.0);
    assert!(near(r["startStation"].as_f64().unwrap(), 1000.0, 0.0));
    // Marks at 1+000 ... every 100 along the length: a line and a label each.
    let total = r["length"].as_f64().unwrap();
    let marks = (total / 100.0).floor() as usize + 1;
    assert_eq!(layer_count(&s, "ALIGN-STA"), 2 * marks);
    assert!(layer_count(&s, "ALIGN-CURVES") == 2);
}

#[test]
fn impossible_alignments_are_refused_and_change_nothing() {
    let mut s = Session::new();
    for p in [
        json!({}),
        json!({ "pis": [[0, 0]] }),
        json!({ "pis": [[0, 0], [0, 0]] }),
        json!({ "pis": [[0, 0], ["a", 1]] }),
        json!({ "pis": [[0, 0], [1e300, 1]] }),
        json!({ "pis": [[0, 0], [100, 0, -5], [100, 100]] }),
        json!({ "pis": [[0, 0], [100, 0], [100, 100]], "radius": 500 }),
        json!({ "pis": [[0, 0], [100, 0], [0, 1e-7]], "radius": 10 }),
        json!({ "pis": [[0, 0], [100, 0], [100, 100]], "radius": -1 }),
        json!({ "pis": [[0, 0], [100, 0], [100, 100]], "ticks": 1e-9 }),
        json!({ "pis": [[0, 0], [100, 0], [100, 100]], "name": "a<b" }),
    ] {
        assert!(s.execute("roadalign", &p).is_err(), "{p}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), 0);
    s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0]], "name": "Same" })).unwrap();
    assert!(s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0]], "name": "same" })).is_err());
}

#[test]
fn sections_follow_the_arc_of_a_curved_alignment() {
    // A plane z = 0.1 x + 0.02 y.
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
    s.execute("roadalign", &json!({ "pis": [[10, 10], [90, 10], [90, 90]], "radius": 30, "labels": false })).unwrap();
    let r = s.execute("xsection", &json!({ "name": "Line 1A", "interval": 20, "width": 5 })).unwrap();
    let last = r["stations"].as_array().unwrap().last().and_then(|v| v.as_f64()).unwrap();
    // 50 + a quarter circle of radius 30 + 50, not the chord's 142.4.
    assert!(near(last, 100.0 + 30.0 * std::f64::consts::FRAC_PI_2, 0.1), "{last}");
    assert_eq!(r["sections"], 9);
}

#[test]
fn the_pis_can_come_from_a_line_or_polyline_on_any_layer() {
    let mut s = Session::new();
    // A plain polyline on its own layer (here made as a sharp alignment, which is one).
    s.execute("roadalign", &json!({ "pis": [[0, 0], [100, 0], [100, 100]], "radius": 0, "name": "Sketch" })).unwrap();
    let handle = s
        .doc()
        .unwrap()
        .model
        .iter()
        .find(|e| e.common.layer == "CL-Sketch" && matches!(e.kind, EntityKind::LwPolyline(_)))
        .map(|e| e.handle.hex())
        .unwrap();
    let r = s.execute("roadalign", &json!({ "from": handle, "radius": 20, "name": "Road" })).unwrap();
    assert_eq!(r["curves"].as_array().unwrap().len(), 1);
    assert!(near(r["curves"][0]["tangent"].as_f64().unwrap(), 20.0, 1e-9));
    // The same PIs from the sketch by name; the sketch itself stays as it was.
    let r = s.execute("roadalign", &json!({ "fromName": "sketch", "radius": 10, "name": "Road2" })).unwrap();
    assert!(near(r["curves"][0]["tangent"].as_f64().unwrap(), 10.0, 1e-9));
    assert_eq!(layer_count(&s, "CL-Sketch"), 1);
    // A line from a layer of the drawing's own works too.
    s.execute("line", &json!({ "points": [[0, 0], [50, 0]] })).unwrap();
    let line = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle.hex()).unwrap();
    assert!(s.execute("roadalign", &json!({ "from": line, "name": "L" })).is_ok());
    for p in [json!({ "from": "ZZ" }), json!({ "from": "1" }), json!({ "fromName": "nothing" })] {
        assert!(s.execute("roadalign", &p).is_err(), "{p}");
    }
}

#[test]
fn the_ui_can_take_the_pis_for_its_dialog_instead_of_the_command_line_questions() {
    let mut s = Session::new();
    s.align_dialog = true;
    assert!(s.start("roadalign").is_ok());
    for t in ["0,0", "100,0", "100,100"] {
        assert!(s.cmdline(t).is_ok());
    }
    assert!(s.cmdline("").is_ok());
    let pis = s.align_pending.take().unwrap_or_default();
    assert_eq!(pis.len(), 3);
    assert!(s.running.is_none(), "the command ended");
    assert_eq!(layer_count(&s, "CL-Line 1A"), 0, "nothing is drawn until the dialog's OK");
    // Without the flag it asks at the command line, as before.
    assert!(s.start("roadalign").is_ok());
    for t in ["0,0", "100,0", ""] {
        assert!(s.cmdline(t).is_ok());
    }
    assert!(s.running.is_some() && s.align_pending.is_none());
}

#[test]
fn an_existing_line_can_be_selected_at_the_start_prompt() {
    let mut s = Session::new();
    assert!(s.execute("line", &json!({ "points": [[0, 0], [100, 0], [100, 100]] })).is_ok());
    assert!(s.start("roadalign").is_ok());
    assert!(s.cmdline("select").is_ok());
    assert!(s.cmdline("last").is_ok());
    assert!(s.cmdline("100").is_ok(), "radius");
    assert!(s.cmdline("Typed").is_ok(), "name");
    assert_eq!(layer_count(&s, "CL-Typed"), 1);
}
