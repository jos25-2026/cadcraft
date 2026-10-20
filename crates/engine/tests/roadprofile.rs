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

fn hatch_area(s: &Session, layer: &str) -> (usize, f64) {
    let mut total = 0.0;
    let mut n = 0;
    for e in s.doc().unwrap().model.iter().filter(|e| e.common.layer == layer) {
        if let EntityKind::Hatch(h) = &e.kind {
            n += 1;
            for l in &h.loops {
                let v = &l.vertices;
                let a: f64 = (0..v.len()).map(|i| v[i].p.x * v[(i + 1) % v.len()].p.y - v[(i + 1) % v.len()].p.x * v[i].p.y).sum();
                total += a.abs() / 2.0;
            }
        }
    }
    (n, total)
}

#[test]
fn cut_and_fill_are_shaded_and_the_shading_matches_the_areas() {
    let mut s = site();
    let r = s.execute("roadsection", &json!({ "name": "Road", "pvis": [[0, 5, 0], [80, 5, 0]], "interval": 20 })).unwrap();
    let secs = r["sections"].as_array().unwrap();
    let cut: f64 = secs.iter().map(|x| x["cut"].as_f64().unwrap()).sum();
    let fill: f64 = secs.iter().map(|x| x["fill"].as_f64().unwrap()).sum();
    let (nc, ac) = hatch_area(&s, "RS-CUT");
    let (nf, af) = hatch_area(&s, "RS-FILL");
    assert!(nc >= 3 && nf >= 1, "cut hatches {nc}, fill hatches {nf}");
    // The default sample step on the sheet is the one the areas use, so the hatches add up to them.
    assert!((ac - cut).abs() < 1e-6 * cut.max(1.0), "{ac} vs {cut}");
    assert!((af - fill).abs() < 1e-6 * fill.max(1.0), "{af} vs {fill}");
    // Off: no hatches, same numbers.
    let mut s2 = site();
    let r2 = s2.execute("roadsection", &json!({ "name": "Road", "pvis": [[0, 5, 0], [80, 5, 0]], "interval": 20, "shade": false })).unwrap();
    assert_eq!(hatch_area(&s2, "RS-CUT").0 + hatch_area(&s2, "RS-FILL").0, 0);
    assert_eq!(r2["cutVolume"], r["cutVolume"]);
}

#[test]
fn a_ditch_deepens_only_the_cut_sides() {
    let mut s = site();
    let pv = json!([[0, 5, 0], [80, 5, 0]]);
    let a = s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 40 })).unwrap();
    let b = s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 40, "ditchDepth": 0.5, "layerPrefix": "RD" })).unwrap();
    let (sa, sb) = (a["sections"].as_array().unwrap(), b["sections"].as_array().unwrap());
    // Station 0 is a fill on both sides: no ditch there.
    assert!((sa[0]["fill"].as_f64().unwrap() - sb[0]["fill"].as_f64().unwrap()).abs() < 1e-9);
    assert_eq!(sa[0]["daylightLeft"], sb[0]["daylightLeft"]);
    // Station 80 is a deep cut: the ditch adds cut area and the daylight moves out.
    assert!(sb[2]["cut"].as_f64().unwrap() > sa[2]["cut"].as_f64().unwrap() + 1.0, "{} vs {}", sb[2]["cut"], sa[2]["cut"]);
    assert!(sb[2]["daylightLeft"].as_f64().unwrap() > sa[2]["daylightLeft"].as_f64().unwrap());
    assert!(b["cutVolume"].as_f64().unwrap() > a["cutVolume"].as_f64().unwrap());
    // The ditch is drawn: shoulder edge, two ditch corners, daylight on each side plus the 5 road points.
    let dsg = s.doc().unwrap().model.iter().filter(|e| e.common.layer == "RD-DESIGN").filter_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) => Some(p.vertices.len()),
        _ => None,
    });
    assert_eq!(dsg.max(), Some(5 + 2 * 3), "a cut section has both ditches");
}

#[test]
fn superelevation_banks_the_road_between_its_rows() {
    let mut s = site();
    let pv = json!([[0, 5, 0], [80, 5, 0]]);
    let se = json!([[20, -2, -2], [40, 6, -6], [60, 6, -6]]);
    let r = s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 10, "superelevation": se })).unwrap();
    let secs = r["sections"].as_array().unwrap();
    let at = |i: usize, k: &str| secs[i][k].as_f64().unwrap();
    // Outside the table, and at its first row: the normal crown.
    assert!((at(0, "slopeLeft") + 2.0).abs() < 1e-9 && (at(0, "slopeRight") + 2.0).abs() < 1e-9);
    assert!((at(8, "slopeLeft") + 2.0).abs() < 1e-9);
    // Halfway up the transition, then fully banked.
    assert!((at(3, "slopeLeft") - 2.0).abs() < 1e-9 && (at(3, "slopeRight") + 4.0).abs() < 1e-9, "{}", secs[3]);
    assert!((at(4, "slopeLeft") - 6.0).abs() < 1e-9 && (at(4, "slopeRight") + 6.0).abs() < 1e-9);
    assert!((at(6, "slopeRight") + 6.0).abs() < 1e-9);
    // Banking moves earth: the areas differ from the crowned road at a banked station, not at a crowned one.
    let c = s.execute("roadsection", &json!({ "name": "Road", "pvis": pv, "interval": 10, "layerPrefix": "RC" })).unwrap();
    let cs = c["sections"].as_array().unwrap();
    assert!((cs[0]["cut"].as_f64().unwrap() - at(0, "cut")).abs() < 1e-9);
    assert!((cs[4]["cut"].as_f64().unwrap() - at(4, "cut")).abs() > 1e-6 || (cs[4]["fill"].as_f64().unwrap() - at(4, "fill")).abs() > 1e-6);
    assert!(secs[4]["slopeLeft"].as_f64().unwrap() > cs[4]["slopeLeft"].as_f64().unwrap());
}

#[test]
fn bad_ditches_and_superelevations_are_refused() {
    let mut s = site();
    let pv = json!([[0, 5, 0], [80, 5, 0]]);
    let n = s.doc().unwrap().model.iter().count();
    for bad in [
        json!({"ditchDepth": -1}),
        json!({"ditchSlope": 0}),
        json!({"ditchWidth": -1}),
        json!({"superelevation": "x"}),
        json!({"superelevation": [[0, 2]]}),
        json!({"superelevation": [[0, 2, 2], [0, 3, 3]]}),
        json!({"superelevation": [[0, 40, -40], [10, 2, 2]]}),
        json!({"superelevation": [[1e30, 2, 2]]}),
    ] {
        let mut p = json!({ "name": "Road", "pvis": pv });
        for (k, v) in bad.as_object().unwrap() {
            p[k] = v.clone();
        }
        assert!(s.execute("roadsection", &p).is_err(), "{bad}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), n);
}
