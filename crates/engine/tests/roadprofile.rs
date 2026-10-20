//! ROADPROFILE (ground, design grade and vertical curves) and ROADSECTION (road template, cut and fill).

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

/// A 100 x 100 grid of points on the plane z = 0.1 x + 0.02 y, and a centerline "Road" along y = 50.
fn site() -> Session {
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
    s.execute("alignment", &json!({ "points": [[10, 50], [90, 50]], "name": "Road" })).unwrap();
    s
}

fn layer_count(s: &Session, layer: &str) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.common.layer == layer).count()
}

#[test]
fn a_crest_curve_has_the_textbook_elements() {
    let mut s = site();
    let r = s.execute("roadprofile", &json!({ "name": "Road", "pvis": [[0, 2, 0], [40, 6, 40], [80, 6, 0]] })).unwrap();
    let c = &r["curves"][0];
    assert_eq!(c["kind"], "crest");
    assert!((c["pvc"].as_f64().unwrap() - 20.0).abs() < 1e-9 && (c["pvt"].as_f64().unwrap() - 60.0).abs() < 1e-9);
    assert!((c["pvcElevation"].as_f64().unwrap() - 4.0).abs() < 1e-9, "{c}");
    assert!((c["pvtElevation"].as_f64().unwrap() - 6.0).abs() < 1e-9);
    assert!((c["k"].as_f64().unwrap() - 4.0).abs() < 1e-9, "K = L / A(%) = 40 / 10");
    assert!((c["midElevation"].as_f64().unwrap() - 5.5).abs() < 1e-9, "the curve sits A L / 8 below the PVI");
    assert!((c["turningElevation"].as_f64().unwrap() - 6.0).abs() < 1e-9);
    assert!((r["grades"][0]["percent"].as_f64().unwrap() - 10.0).abs() < 1e-9);
    assert_eq!(layer_count(&s, "PR-GROUND"), 1);
    assert_eq!(layer_count(&s, "PR-DESIGN"), 1);
    assert_eq!(layer_count(&s, "PVI-Road"), 3, "the PVIs are kept as text");
    // Running it again replaces the kept PVIs, not adds to them.
    s.execute("roadprofile", &json!({ "name": "Road", "pvis": [[0, 2, 0], [80, 8, 0]] })).unwrap();
    assert_eq!(layer_count(&s, "PVI-Road"), 2);
}

#[test]
fn a_sag_curve_is_marked_and_the_pvis_come_back_from_the_drawing() {
    let mut s = site();
    s.execute("roadprofile", &json!({ "name": "Road", "pvis": [[0, 6, 0], [40, 2, 20], [80, 6, 0]] })).unwrap();
    let r = s.execute("roadprofile", &json!({ "name": "Road" })).unwrap();
    assert_eq!(r["curves"][0]["kind"], "sag");
    assert_eq!(r["pvis"].as_array().unwrap().len(), 3);
}

#[test]
fn bad_grades_are_refused_and_change_nothing() {
    let mut s = site();
    let n = s.doc().unwrap().model.iter().count();
    for pvis in [
        json!([[0, 2, 0]]),
        json!([[0, 2, 0], [0, 3, 0]]),
        json!([[0, 2, 0], [10, 3, 30], [20, 3, 30], [80, 4, 0]]),
        json!([[0, 2, 0], [10, "x", 0]]),
        json!([[0, 2, -5], [10, 3, 0]]),
        json!([[0, 2, 0], [1e30, 3, 0]]),
        json!("nope"),
    ] {
        assert!(s.execute("roadprofile", &json!({ "name": "Road", "pvis": pvis })).is_err(), "{pvis}");
    }
    assert!(s.execute("roadprofile", &json!({ "name": "Road", "vScale": -1 })).is_err());
    assert!(s.execute("roadprofile", &json!({ "name": "Nobody" })).is_err());
    assert!(s.execute("roadsection", &json!({ "name": "Road" })).is_err(), "no design grade yet");
    assert_eq!(s.doc().unwrap().model.iter().count(), n);
}

#[test]
fn road_sections_find_the_daylight_and_the_cut_and_fill() {
    let mut s = site();
    s.execute("roadprofile", &json!({ "name": "Road", "pvis": [[0, 5, 0], [80, 5, 0]] })).unwrap();
    // The PVIs kept by ROADPROFILE are the design grade.
    let r = s.execute("roadsection", &json!({ "name": "Road", "interval": 20 })).unwrap();
    let secs = r["sections"].as_array().unwrap();
    assert_eq!(secs.len(), 5);
    // Station 40: ground 6.0 on the centerline, 5.0 design: a cut. Daylight where
    // 4.87 + d / 1.5 meets the ground that rises 2 % to the left and falls 2 % to the right (the left side slope runs uphill).
    let mid = &secs[2];
    assert!((mid["ground"].as_f64().unwrap() - 6.0).abs() < 1e-6 && (mid["design"].as_f64().unwrap() - 5.0).abs() < 1e-9);
    assert!((mid["daylightLeft"].as_f64().unwrap() - (5.0 + 1.23 / (1.0 / 1.5 - 0.02))).abs() < 1e-3, "{mid}");
    assert!((mid["daylightRight"].as_f64().unwrap() - (5.0 + 1.03 / (1.0 / 1.5 + 0.02))).abs() < 1e-3, "{mid}");
    let (cut, fill) = (mid["cut"].as_f64().unwrap(), mid["fill"].as_f64().unwrap());
    assert!(cut > 8.0 && cut < 14.0 && fill < 1e-6, "cut {cut} fill {fill}");
    // Station 0 is on a fill (ground 2.0), station 80 deep in a cut (ground 10.0).
    assert!(secs[0]["fill"].as_f64().unwrap() > 5.0 && secs[0]["cut"].as_f64().unwrap() < 1e-6);
    assert!(secs[4]["cut"].as_f64().unwrap() > 30.0);
    // Volumes by average end area.
    let (vc, vf) = (r["cutVolume"].as_f64().unwrap(), r["fillVolume"].as_f64().unwrap());
    assert!(vc > 0.0 && vf > 0.0);
    let by_hand: f64 = secs.windows(2).map(|w| (w[0]["cut"].as_f64().unwrap() + w[1]["cut"].as_f64().unwrap()) / 2.0 * 20.0).sum();
    assert!((vc - by_hand).abs() < 1e-6);
    assert_eq!(layer_count(&s, "RS-GROUND"), 5);
    assert_eq!(layer_count(&s, "RS-DESIGN"), 5);
    assert_eq!(layer_count(&s, "RS-PLAN-EDGE"), 2, "the two road edges on the plan");
    assert_eq!(layer_count(&s, "RS-PLAN-DAYLIGHT"), 2);
    // Every drawn thing is finite.
    for e in s.doc().unwrap().model.iter() {
        if let EntityKind::LwPolyline(pl) = &e.kind {
            assert!(pl.vertices.iter().all(|v| v.p.x.is_finite() && v.p.y.is_finite()));
        }
    }
}

#[test]
fn a_wider_road_and_flatter_slopes_change_the_result() {
    let mut s = site();
    let pv = json!([[0, 5, 0], [80, 5, 0]]);
    let a = s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 40 })).unwrap();
    let b =
        s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 40, "lanes": 2, "cutSlope": 1.0, "layerPrefix": "RW" })).unwrap();
    assert!(b["cutVolume"].as_f64().unwrap() > a["cutVolume"].as_f64().unwrap(), "a wider road cuts more");
    for bad in
        [json!({"lanes": 0}), json!({"lanes": 99}), json!({"laneWidth": -1}), json!({"cutSlope": 0}), json!({"interval": 0}), json!({"reach": "far"})]
    {
        let mut p = json!({ "name": "Road", "pvis": pv });
        for (k, v) in bad.as_object().unwrap() {
            p[k] = v.clone();
        }
        assert!(s.execute("roadsection", &p).is_err(), "{bad}");
    }
}
