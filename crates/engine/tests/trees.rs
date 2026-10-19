//! TREES: symbols at the points of the chosen layers, and one layer per description code.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use serde_json::json;

const FILE: &str = "1,1000,2000,10,TREE big\n2,1010,2005,11,tree\n3,1020,2010,12,PALM\n4,1030,2020,13,\n5,1040,2030,14,FENCE POST\n";

fn count(s: &Session, layer: &str) -> usize {
    s.doc().map(|d| d.model.iter().filter(|e| e.common.layer == layer).count()).unwrap_or(0)
}

#[test]
fn each_code_gets_its_own_point_layer() {
    let mut s = Session::new();
    let r = s.execute("importpoints", &json!({ "text": FILE, "layerByCode": true })).unwrap();
    assert_eq!(r["codeLayers"], json!(["SURVEY-PTS-FENCE", "SURVEY-PTS-PALM", "SURVEY-PTS-TREE"]));
    let pts = |l: &str| s.doc().unwrap().model.iter().filter(|e| e.common.layer == l && matches!(e.kind, EntityKind::Point(_))).count();
    assert_eq!((pts("SURVEY-PTS-TREE"), pts("SURVEY-PTS-PALM"), pts("SURVEY-PTS")), (2, 1, 1), "no code stays on the base layer");
}

#[test]
fn trees_are_planted_on_the_chosen_layers_in_every_style() {
    for (style, per_tree) in [("round", 4), ("cloud", 9), ("conifer", 3), ("palm", 7)] {
        let mut s = Session::new();
        s.execute("importpoints", &json!({ "text": FILE, "layerByCode": true })).unwrap();
        let r = s.execute("trees", &json!({ "layers": ["survey-pts-tree", "SURVEY-PTS-PALM"], "style": style, "size": 6 })).unwrap();
        assert_eq!(r["planted"], 3, "{style}");
        assert_eq!(count(&s, "TREES"), 3 * per_tree, "{style}");
    }
}

#[test]
fn bad_trees_requests_are_refused_not_panicked() {
    let mut s = Session::new();
    s.execute("importpoints", &json!({ "text": FILE, "layerByCode": true })).unwrap();
    for p in [
        json!({}),
        json!({ "layers": [] }),
        json!({ "layers": ["NOPE"] }),
        json!({ "layers": [1, null] }),
        json!({ "layers": ["SURVEY-PTS-TREE"], "size": -1 }),
        json!({ "layers": ["SURVEY-PTS-TREE"], "size": 1e300 }),
        json!({ "layers": ["SURVEY-PTS-TREE"], "style": "oak" }),
        json!({ "layers": ["SURVEY-PTS-TREE"], "layer": "a<b" }),
    ] {
        assert!(s.execute("trees", &p).is_err(), "{p}");
    }
    assert_eq!(count(&s, "TREES"), 0);
}

#[test]
fn north_arrow_styles() {
    for style in ["classic", "full", "simple", "compass"] {
        let mut s = Session::new();
        s.execute("northarrow", &json!({ "at": [0, 0], "size": 10, "style": style })).unwrap();
        assert!(count(&s, "NORTH") >= 3, "{style}");
    }
    assert!(Session::new().execute("northarrow", &json!({ "at": [0, 0], "style": "fancy" })).is_err());
}

#[test]
fn trees_take_a_colour() {
    use cadcraft_color::{Color, Rgb};
    for (v, want) in [(json!("#10a030"), Color::True(Rgb(16, 160, 48))), (json!([1, 2, 3]), Color::True(Rgb(1, 2, 3))), (json!(3), Color::Index(3))] {
        let mut s = Session::new();
        s.execute("importpoints", &json!({ "text": FILE, "layerByCode": true })).unwrap();
        s.execute("trees", &json!({ "layers": ["SURVEY-PTS-TREE"], "color": v })).unwrap();
        let d = s.doc().unwrap();
        assert!(d.model.iter().filter(|e| e.common.layer == "TREES").all(|e| e.common.color == want), "{v}");
    }
    let mut s = Session::new();
    s.execute("importpoints", &json!({ "text": FILE, "layerByCode": true })).unwrap();
    for bad in [json!("red"), json!("#12345"), json!("#zzzzzz"), json!([1, 2]), json!([1, 2, 300]), json!(0), json!(256), json!(true)] {
        assert!(s.execute("trees", &json!({ "layers": ["SURVEY-PTS-TREE"], "color": bad })).is_err(), "{bad}");
    }
}
