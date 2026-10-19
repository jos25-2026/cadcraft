//! Dialogs for the survey tools: Import Points, Earthwork, Map Grid and North Arrow. The icon (or the
//! typed command without parameters) opens one; OK runs the command with the fields. An empty
//! field means "automatic". JSON calls and `-` forms keep running the command directly.

use egui::vec2;
use serde_json::{Map, Value, json};

use crate::CadApp;

/// Empty = automatic; otherwise a positive finite number.
fn positive(text: &str) -> Result<Option<f64>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0).map(Some).ok_or(())
}

/// Empty = automatic; otherwise any finite number.
fn number(text: &str) -> Result<Option<f64>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite()).map(Some).ok_or(())
}

fn put(
    m: &mut Map<String, Value>,
    key: &str,
    text: &str,
    field: &'static str,
    parse: fn(&str) -> Result<Option<f64>, ()>,
) -> Result<(), &'static str> {
    if let Some(v) = parse(text).map_err(|()| field)? {
        m.insert(key.into(), json!(v));
    }
    Ok(())
}

fn put_text(m: &mut Map<String, Value>, key: &str, text: &str) {
    if !text.trim().is_empty() {
        m.insert(key.into(), json!(text.trim()));
    }
}

fn edit(ui: &mut egui::Ui, text: &mut String, auto: bool) {
    let mut e = egui::TextEdit::singleline(text).desired_width(140.0);
    if auto {
        e = e.hint_text(crate::tl!("Automatic"));
    }
    ui.add(e);
}

/// One dialog: the grid of fields from `fields`, an error line, OK and Cancel. `submit` runs on OK
/// and closes the dialog when it succeeds; `after` handles buttons inside the fields (Browse).
#[allow(clippy::too_many_arguments)]
fn shell<F: Clone + Default + Send + Sync + 'static>(
    app: &mut CadApp,
    ctx: &egui::Context,
    open: &mut bool,
    key: &str,
    title: &str,
    fields: impl FnOnce(&mut egui::Ui, &mut F),
    after: impl FnOnce(&mut CadApp, &mut F),
    submit: impl FnOnce(&mut CadApp, &F) -> Result<(), String>,
) {
    let id = egui::Id::new((key, "form"));
    let err_id = egui::Id::new((key, "error"));
    let mut f = ctx.data_mut(|d| d.get_temp::<F>(id)).unwrap_or_default();
    let mut err = ctx.data_mut(|d| d.get_temp::<String>(err_id)).unwrap_or_default();
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new(title).id(egui::Id::new(key)).open(open).resizable(false).collapsible(false).show(ctx, |ui| {
        egui::Grid::new((key, "grid")).num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| fields(ui, &mut f));
        if !err.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &err);
        }
        ui.horizontal(|ui| {
            ok = ui.button(crate::tl!("OK")).clicked();
            cancel = ui.button(crate::tl!("Cancel")).clicked();
        });
    });
    after(app, &mut f);
    if ok {
        match submit(app, &f) {
            Ok(()) => {
                err.clear();
                *open = false;
            }
            Err(e) => err = e,
        }
    }
    if cancel {
        *open = false;
    }
    ctx.data_mut(|d| {
        d.insert_temp(id, f);
        d.insert_temp(err_id, err);
    });
}

fn bad_field(field: &str) -> String {
    format!("{}: {}", crate::i18n::t(field), crate::tl!("enter a positive number"))
}

fn run(app: &mut CadApp, command: &str, params: Result<Value, &'static str>, zoom: bool) -> Result<(), String> {
    let p = params.map_err(bad_field)?;
    app.run(command, p)?;
    if zoom {
        let _ = app.session.execute("zoom.extents", &Value::Null);
    }
    Ok(())
}

// ---- Import Points ----

#[derive(Clone)]
struct PointsForm {
    path: String,
    format: String,
    text_height: String,
    decimals: u32,
    prefix: String,
    number: bool,
    elevation: bool,
    description: bool,
    by_code: bool,
    browse: bool,
}

impl Default for PointsForm {
    fn default() -> Self {
        PointsForm {
            path: String::new(),
            format: String::new(),
            text_height: String::new(),
            decimals: 2,
            prefix: String::new(),
            number: true,
            elevation: true,
            description: true,
            by_code: false,
            browse: false,
        }
    }
}

fn points_params(f: &PointsForm) -> Result<Value, &'static str> {
    let mut m = Map::new();
    if f.path.trim().is_empty() {
        return Err("Point file");
    }
    put_text(&mut m, "path", f.path.trim().trim_matches('"'));
    put_text(&mut m, "format", &f.format.to_ascii_uppercase());
    put(&mut m, "textHeight", &f.text_height, "Text height", positive)?;
    m.insert("decimals".into(), json!(f.decimals));
    put_text(&mut m, "layerPrefix", &f.prefix);
    if f.by_code {
        m.insert("layerByCode".into(), json!(true));
    }
    // Labels stay automatic (off for a big grid of points) unless one is switched off here.
    for (key, on) in [("showNumber", f.number), ("showElevation", f.elevation), ("showDescription", f.description)] {
        if !on {
            m.insert(key.into(), json!(false));
        }
    }
    Ok(Value::Object(m))
}

pub fn points(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    shell::<PointsForm>(
        app,
        ctx,
        open,
        "points_dialog",
        crate::tl!("Import Survey Points"),
        |ui, f| {
            ui.label(crate::tl!("Point file"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut f.path).desired_width(220.0));
                f.browse = ui.button(crate::tl!("Browse...")).clicked();
            });
            ui.end_row();
            ui.label(crate::tl!("Column order"));
            edit(ui, &mut f.format, true);
            ui.end_row();
            ui.label(crate::tl!("Text height"));
            edit(ui, &mut f.text_height, true);
            ui.end_row();
            ui.label(crate::tl!("Decimals"));
            ui.add(egui::DragValue::new(&mut f.decimals).range(0..=8));
            ui.end_row();
            ui.label(crate::tl!("Layer prefix"));
            edit(ui, &mut f.prefix, false);
            ui.end_row();
            ui.label(crate::tl!("Separate layer for each code"));
            ui.checkbox(&mut f.by_code, "");
            ui.end_row();
            ui.label(crate::tl!("Point numbers"));
            ui.checkbox(&mut f.number, "");
            ui.end_row();
            ui.label(crate::tl!("Elevations"));
            ui.checkbox(&mut f.elevation, "");
            ui.end_row();
            ui.label(crate::tl!("Descriptions"));
            ui.checkbox(&mut f.description, "");
            ui.end_row();
        },
        |app, f| {
            if std::mem::take(&mut f.browse)
                && let Some(p) = app.services.pick_file.as_ref().and_then(|pick| pick("points"))
            {
                f.path = p;
            }
        },
        |app, f| run(app, "importpoints", points_params(f), true),
    );
}

// ---- Earthwork ----

#[derive(Clone)]
struct EarthForm {
    elevation: String,
    label: bool,
    text_height: String,
    layer: String,
}

impl Default for EarthForm {
    fn default() -> Self {
        EarthForm { elevation: String::new(), label: true, text_height: String::new(), layer: String::new() }
    }
}

fn earth_params(f: &EarthForm) -> Result<Value, &'static str> {
    let mut m = Map::new();
    put(&mut m, "elevation", &f.elevation, "Design elevation", number)?;
    m.insert("label".into(), json!(f.label));
    put(&mut m, "textHeight", &f.text_height, "Label text height", positive)?;
    put_text(&mut m, "layer", &f.layer);
    Ok(Value::Object(m))
}

pub fn earthwork(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    shell::<EarthForm>(
        app,
        ctx,
        open,
        "earthwork_dialog",
        crate::tl!("Earthwork Volumes"),
        |ui, f| {
            ui.label(crate::tl!("Design elevation"));
            edit(ui, &mut f.elevation, true);
            ui.end_row();
            ui.label(crate::tl!("Write result into the drawing"));
            ui.checkbox(&mut f.label, "");
            ui.end_row();
            ui.label(crate::tl!("Label text height"));
            ui.add_enabled_ui(f.label, |ui| edit(ui, &mut f.text_height, true));
            ui.end_row();
            ui.label(crate::tl!("Layer"));
            ui.add_enabled_ui(f.label, |ui| edit(ui, &mut f.layer, false));
            ui.end_row();
        },
        |_, _| {},
        |app, f| run(app, "earthwork", earth_params(f), false),
    );
}

// ---- Map Grid ----

#[derive(Clone)]
struct GridForm {
    spacing: String,
    cross: bool,
    text_height: String,
    decimals: u32,
    prefix_labels: bool,
    frame: bool,
    thousands: bool,
    layer_prefix: String,
}

impl Default for GridForm {
    fn default() -> Self {
        GridForm {
            spacing: String::new(),
            cross: false,
            text_height: String::new(),
            decimals: 0,
            prefix_labels: true,
            frame: true,
            thousands: false,
            layer_prefix: String::new(),
        }
    }
}

fn grid_params(f: &GridForm) -> Result<Value, &'static str> {
    let mut m = Map::new();
    put(&mut m, "spacing", &f.spacing, "Grid spacing", positive)?;
    m.insert("style".into(), json!(if f.cross { "cross" } else { "lines" }));
    put(&mut m, "textHeight", &f.text_height, "Label text height", positive)?;
    m.insert("decimals".into(), json!(f.decimals));
    m.insert("labelPrefix".into(), json!(f.prefix_labels));
    m.insert("frame".into(), json!(f.frame));
    m.insert("thousands".into(), json!(f.thousands));
    put_text(&mut m, "layerPrefix", &f.layer_prefix);
    Ok(Value::Object(m))
}

pub fn mapgrid(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    shell::<GridForm>(
        app,
        ctx,
        open,
        "mapgrid_dialog",
        crate::tl!("Map Grid"),
        |ui, f| {
            ui.label(crate::tl!("Grid spacing"));
            edit(ui, &mut f.spacing, true);
            ui.end_row();
            ui.label(crate::tl!("Style"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut f.cross, false, crate::tl!("Lines"));
                ui.selectable_value(&mut f.cross, true, crate::tl!("Crosses"));
            });
            ui.end_row();
            ui.label(crate::tl!("Label text height"));
            edit(ui, &mut f.text_height, true);
            ui.end_row();
            ui.label(crate::tl!("Decimals"));
            ui.add(egui::DragValue::new(&mut f.decimals).range(0..=6));
            ui.end_row();
            ui.label(crate::tl!("E / N before the numbers"));
            ui.checkbox(&mut f.prefix_labels, "");
            ui.end_row();
            ui.label(crate::tl!("Frame"));
            ui.checkbox(&mut f.frame, "");
            ui.end_row();
            ui.label(crate::tl!("Thousands separator"));
            ui.checkbox(&mut f.thousands, "");
            ui.end_row();
            ui.label(crate::tl!("Layer prefix"));
            edit(ui, &mut f.layer_prefix, false);
            ui.end_row();
        },
        |_, _| {},
        |app, f| run(app, "mapgrid", grid_params(f), false),
    );
}

// ---- North Arrow ----

#[derive(Clone)]
struct NorthForm {
    size: String,
    rotation: String,
    layer: String,
    style: String,
}

impl Default for NorthForm {
    fn default() -> Self {
        NorthForm { size: String::new(), rotation: String::new(), layer: String::new(), style: "classic".into() }
    }
}

fn north_params(f: &NorthForm) -> Result<Value, &'static str> {
    let mut m = Map::new();
    put(&mut m, "size", &f.size, "Size", positive)?;
    put(&mut m, "rotation", &f.rotation, "Rotation", number)?;
    put_text(&mut m, "layer", &f.layer);
    m.insert("style".into(), json!(f.style));
    Ok(Value::Object(m))
}

pub fn northarrow(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    shell::<NorthForm>(
        app,
        ctx,
        open,
        "northarrow_dialog",
        crate::tl!("North Arrow"),
        |ui, f| {
            ui.label(crate::tl!("Template"));
            ui.horizontal(|ui| {
                for (key, label) in [
                    ("classic", crate::tl!("Classic")),
                    ("full", crate::tl!("Solid")),
                    ("simple", crate::tl!("Arrow")),
                    ("compass", crate::tl!("Compass")),
                ] {
                    ui.selectable_value(&mut f.style, key.to_string(), label);
                }
            });
            ui.end_row();
            ui.label(crate::tl!("Size"));
            edit(ui, &mut f.size, true);
            ui.end_row();
            ui.label(crate::tl!("Rotation"));
            ui.add(egui::TextEdit::singleline(&mut f.rotation).hint_text("0").desired_width(140.0));
            ui.end_row();
            ui.label(crate::tl!("Layer"));
            edit(ui, &mut f.layer, false);
            ui.end_row();
        },
        |_, _| {},
        |app, f| run(app, "northarrow", north_params(f), false),
    );
}

// ---- Plant Trees ----

#[derive(Clone)]
struct TreesForm {
    /// Layers holding POINTs: name, how many, chosen.
    layers: Vec<(String, usize, bool)>,
    size: String,
    style: String,
    layer: String,
    color: [u8; 3],
    refresh: bool,
}

impl Default for TreesForm {
    fn default() -> Self {
        TreesForm { layers: Vec::new(), size: String::new(), style: "round".into(), layer: String::new(), color: [34, 139, 34], refresh: false }
    }
}

/// Every layer of the drawing with the number of POINT objects on it. Layers of imported survey
/// points per code (`SURVEY-PTS-TREE`) and ones named like trees start chosen; the layers holding
/// points come first.
fn point_layers(app: &CadApp) -> Vec<(String, usize, bool)> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    let mut counts: std::collections::BTreeMap<String, usize> = d.layers.iter().map(|l| (l.name.clone(), 0)).collect();
    for e in d.model.iter() {
        if matches!(e.kind, cadcraft_doc::EntityKind::Point(_)) {
            *counts.entry(e.common.layer.clone()).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize, bool)> = counts
        .into_iter()
        .map(|(n, c)| {
            let on = c > 0 && n.to_uppercase().contains("TREE");
            (n, c, on)
        })
        .collect();
    v.sort_by_key(|(n, c, _)| (*c == 0, !n.to_uppercase().contains("-PTS-")));
    v
}

fn trees_params(f: &TreesForm) -> Result<Value, String> {
    let chosen: Vec<&str> = f.layers.iter().filter(|(_, _, on)| *on).map(|(n, _, _)| n.as_str()).collect();
    if chosen.is_empty() {
        return Err(crate::tl!("Choose at least one layer with points.").to_string());
    }
    let mut m = Map::new();
    m.insert("layers".into(), json!(chosen));
    put(&mut m, "size", &f.size, "Crown size", positive).map_err(bad_field)?;
    m.insert("style".into(), json!(f.style));
    m.insert("color".into(), json!(f.color));
    put_text(&mut m, "layer", &f.layer);
    Ok(Value::Object(m))
}

/// A picture of a tree template, drawn like the symbol the command makes.
fn tree_preview(p: &egui::Painter, rect: egui::Rect, style: &str, color: egui::Color32) {
    use egui::{Pos2, Shape, Stroke};
    let stroke = Stroke::new(1.5, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.38;
    let at = |x: f32, y: f32| Pos2::new(c.x + x * r, c.y - y * r);
    let poly = |pts: Vec<Pos2>| p.add(Shape::closed_line(pts, stroke));
    match style {
        "cloud" => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let bump = r * 0.3;
                let centre = Pos2::new(c.x + a.cos() * (r - bump), c.y - a.sin() * (r - bump));
                let pts: Vec<Pos2> = (0..=10)
                    .map(|k| {
                        let t = a - std::f32::consts::FRAC_PI_2 - 0.2 + k as f32 * (std::f32::consts::PI + 0.4) / 10.0;
                        Pos2::new(centre.x + t.cos() * bump * 1.05, centre.y - t.sin() * bump * 1.05)
                    })
                    .collect();
                p.add(Shape::line(pts, stroke));
            }
        }
        "conifer" => {
            poly(vec![at(0.0, 1.0), at(-0.87, -0.5), at(0.87, -0.5)]);
            p.line_segment([at(0.0, 1.0), at(0.0, -0.5)], stroke);
        }
        "palm" => {
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::FRAC_PI_3;
                let (u, w) = (egui::vec2(a.cos(), -a.sin()), egui::vec2(a.sin(), a.cos()));
                poly(vec![c, c + u * (r * 0.5) + w * (r * 0.18), c + u * r, c + u * (r * 0.5) - w * (r * 0.18)]);
            }
        }
        _ => {
            p.circle_stroke(c, r, stroke);
            p.line_segment([at(-0.15, 0.0), at(0.15, 0.0)], stroke);
            p.line_segment([at(0.0, -0.15), at(0.0, 0.15)], stroke);
        }
    }
    p.circle_stroke(c, r * 0.08, stroke);
}

pub fn trees(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    // Closed and reopened (a frame passed without the dialog): start fresh and look at the drawing again.
    let id = egui::Id::new(("trees_dialog", "form"));
    let seen_id = egui::Id::new(("trees_dialog", "seen"));
    let frame = ctx.cumulative_frame_nr();
    let last = ctx.data_mut(|d| d.get_temp::<u64>(seen_id)).unwrap_or(0);
    let refresh = ctx.data_mut(|d| d.get_temp::<TreesForm>(id)).is_none_or(|f| f.refresh);
    if frame > last + 2 || refresh {
        let mut f = if refresh { ctx.data_mut(|d| d.get_temp::<TreesForm>(id)).unwrap_or_default() } else { TreesForm::default() };
        let old = std::mem::take(&mut f.layers);
        f.layers =
            point_layers(app).into_iter().map(|(n, c, on)| (n.clone(), c, old.iter().find(|(o, _, _)| *o == n).map_or(on, |(_, _, k)| *k))).collect();
        f.refresh = false;
        ctx.data_mut(|d| d.insert_temp(id, f));
    }
    ctx.data_mut(|d| d.insert_temp(seen_id, frame));
    shell::<TreesForm>(
        app,
        ctx,
        open,
        "trees_dialog",
        crate::tl!("Plant Trees"),
        |ui, f| {
            ui.label(crate::tl!("Tree layers"));
            ui.vertical(|ui| {
                if f.layers.is_empty() {
                    ui.label(crate::tl!("No layers yet: import survey points first."));
                }
                egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                    for (name, count, on) in &mut f.layers {
                        ui.checkbox(on, format!("{name} ({count})"));
                    }
                });
                f.refresh = ui.small_button(crate::tl!("Refresh")).clicked();
            });
            ui.end_row();
            ui.label(crate::tl!("Template"));
            ui.horizontal(|ui| {
                let color = egui::Color32::from_rgb(f.color[0], f.color[1], f.color[2]);
                for (key, label) in
                    [("round", crate::tl!("Round")), ("cloud", crate::tl!("Cloud")), ("conifer", crate::tl!("Conifer")), ("palm", crate::tl!("Palm"))]
                {
                    ui.vertical(|ui| {
                        let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 64.0), egui::Sense::click());
                        let on = f.style == key;
                        let fill = if on { ui.visuals().selection.bg_fill.gamma_multiply(0.5) } else { ui.visuals().extreme_bg_color };
                        ui.painter().rect_filled(rect, 4.0, fill);
                        if on || resp.hovered() {
                            ui.painter().rect_stroke(rect, 4.0, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
                        }
                        tree_preview(ui.painter(), rect, key, color);
                        if resp.clicked() {
                            f.style = key.to_string();
                        }
                        ui.label(label);
                    });
                }
            });
            ui.end_row();
            ui.label(crate::tl!("Color"));
            ui.color_edit_button_srgb(&mut f.color);
            ui.end_row();
            ui.label(crate::tl!("Crown size"));
            edit(ui, &mut f.size, true);
            ui.end_row();
            ui.label(crate::tl!("Layer"));
            edit(ui, &mut f.layer, false);
            ui.end_row();
        },
        |_, _| {},
        |app, f| {
            let p = trees_params(f)?;
            app.run("trees", p)?;
            Ok(())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fields_are_automatic_and_bad_ones_are_named() {
        assert_eq!(earth_params(&EarthForm::default()).unwrap_or_default().get("elevation"), None);
        let f = EarthForm { elevation: "-3.5".into(), ..EarthForm::default() };
        assert_eq!(earth_params(&f).unwrap_or_default()["elevation"], json!(-3.5));
        let f = EarthForm { elevation: "abc".into(), ..EarthForm::default() };
        assert_eq!(earth_params(&f), Err("Design elevation"));
        let f = EarthForm { text_height: "0".into(), ..EarthForm::default() };
        assert_eq!(earth_params(&f), Err("Label text height"));

        let g = grid_params(&GridForm { cross: true, ..GridForm::default() }).unwrap_or_default();
        assert_eq!((g["style"].as_str(), g.get("spacing")), (Some("cross"), None));
        assert_eq!(grid_params(&GridForm { spacing: "-5".into(), ..GridForm::default() }), Err("Grid spacing"));

        let n = north_params(&NorthForm { rotation: "-30".into(), ..NorthForm::default() }).unwrap_or_default();
        assert_eq!(n["rotation"], json!(-30.0));
        assert_eq!(north_params(&NorthForm { size: "x".into(), ..NorthForm::default() }), Err("Size"));
    }

    #[test]
    fn trees_need_a_chosen_layer_and_a_good_size() {
        let mut f = TreesForm { layers: vec![("SURVEY-PTS-TREE".into(), 4, false)], ..TreesForm::default() };
        assert!(trees_params(&f).is_err(), "nothing chosen");
        f.layers[0].2 = true;
        let p = trees_params(&f).unwrap_or_default();
        assert_eq!((p["layers"].clone(), p["style"].as_str(), p.get("size")), (json!(["SURVEY-PTS-TREE"]), Some("round"), None));
        assert_eq!(p["color"], json!([34, 139, 34]));
        f.size = "-2".into();
        assert!(trees_params(&f).is_err());
    }

    #[test]
    fn point_file_is_required_and_labels_stay_automatic_unless_switched_off() {
        assert_eq!(points_params(&PointsForm::default()), Err("Point file"));
        let f = PointsForm { path: "\"/tmp/a.csv\"".into(), format: "pnezd".into(), ..PointsForm::default() };
        let p = points_params(&f).unwrap_or_default();
        assert_eq!((p["path"].as_str(), p["format"].as_str(), p.get("showNumber")), (Some("/tmp/a.csv"), Some("PNEZD"), None));
        let f = PointsForm { path: "a.csv".into(), elevation: false, ..PointsForm::default() };
        assert_eq!(points_params(&f).unwrap_or_default()["showElevation"], json!(false));
    }
}
