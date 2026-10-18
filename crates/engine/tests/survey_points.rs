//! IMPORTPOINTS: survey point files become POINT entities with number, elevation and
//! description text on their own layers.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

fn count(s: &Session, layer: &str, pred: fn(&EntityKind) -> bool) -> usize {
    let d = s.doc().unwrap();
    d.model.iter().filter(|e| e.common.layer == layer && pred(&e.kind)).count()
}

const FILE: &str = "Pt,N,E,Z,Desc\n1,1000.5,2000.25,10.5,CORNER POST\n2,1010,2005,,TREE\n3,1020,2010,12.345,\n";

#[test]
fn points_and_labels_land_on_their_own_layers() {
    let mut s = Session::new();
    let r = s.execute("importpoints", &json!({ "text": FILE, "textHeight": 1.5 })).unwrap();
    assert_eq!(r["imported"], 3);
    assert_eq!(r["skipped"], 1, "the header line");
    assert_eq!(r["bounds"]["min"], json!([2000.25, 1000.5]), "x is the easting, y the northing");

    let is_pt: fn(&EntityKind) -> bool = |k| matches!(k, EntityKind::Point(_));
    let is_tx: fn(&EntityKind) -> bool = |k| matches!(k, EntityKind::Text(_));
    assert_eq!(count(&s, "SURVEY-PTS", is_pt), 3);
    assert_eq!(count(&s, "SURVEY-NO", is_tx), 3);
    assert_eq!(count(&s, "SURVEY-ELEV", is_tx), 2, "point 2 has no elevation");
    assert_eq!(count(&s, "SURVEY-DESC", is_tx), 2, "point 3 has no description");

    // The point sits at (E, N, Z) and the elevation reads with two decimals.
    let d = s.doc().unwrap();
    let p1 = d.model.iter().find_map(|e| match &e.kind {
        EntityKind::Point(p) if (p.p.x - 2000.25).abs() < 1e-9 => Some(p.p),
        _ => None,
    });
    assert_eq!(p1.map(|p| (p.y, p.z)), Some((1000.5, 10.5)));
    let elevs: Vec<String> = d
        .model
        .iter()
        .filter(|e| e.common.layer == "SURVEY-ELEV")
        .filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None })
        .collect();
    assert!(elevs.contains(&"12.35".to_string()) || elevs.contains(&"12.34".to_string()), "{elevs:?}");
}

#[test]
fn one_undo_removes_the_whole_import() {
    let mut s = Session::new();
    let before = s.doc().unwrap().model.iter().count();
    s.execute("importpoints", &json!({ "text": FILE })).unwrap();
    assert!(s.doc().unwrap().model.iter().count() > before);
    s.cmdline("u").unwrap();
    assert_eq!(s.doc().unwrap().model.iter().count(), before);
}

#[test]
fn format_layers_and_switches_are_honoured() {
    let mut s = Session::new();
    s.execute(
        "importpoints",
        &json!({ "text": "500 600 7 FENCE\n", "format": "ENZD", "layerPrefix": "TOPO", "showNumber": false, "showDescription": false }),
    )
    .unwrap();
    let d = s.doc().unwrap();
    assert!(d.layer("TOPO-PTS").is_some());
    assert!(d.layer("SURVEY-PTS").is_none());
    assert_eq!(d.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count(), 1, "only the elevation");
}

#[test]
fn bad_input_is_an_error_and_changes_nothing() {
    let mut s = Session::new();
    let before = s.doc().unwrap().model.iter().count();
    for params in [
        json!({}),
        json!({ "text": "" }),
        json!({ "text": "a,b,c" }),
        json!({ "text": "1,2,3", "format": "PNZ" }),
        json!({ "text": "1,2,3", "format": "PNEX" }),
        json!({ "text": "1,2,3", "textHeight": 0 }),
        json!({ "text": "1,2,3", "decimals": 99 }),
        json!({ "text": "1,2,3", "layerPrefix": "A/B" }),
        json!({ "path": "/nonexistent/points.csv" }),
    ] {
        assert!(s.execute("importpoints", &params).is_err(), "{params}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), before);
}

#[test]
fn reads_a_file_by_path() {
    let dir = std::env::temp_dir().join(format!("cadcraft-survey-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pts.txt");
    // Latin-1 bytes in a description must not stop the import.
    std::fs::write(&path, b"1 100 200 5 caf\xe9\n").unwrap();
    let mut s = Session::new();
    let r = s.execute("importpoints", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(r["imported"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}
