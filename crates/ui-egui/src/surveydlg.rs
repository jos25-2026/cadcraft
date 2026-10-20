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

// ---- Cross Sections ----

#[derive(Clone)]
struct XsForm {
    name: String,
    interval: String,
    left: String,
    right: String,
    vscale: String,
    columns: u32,
    grid: bool,
    grid_every: String,
    vgrid: bool,
    vgrid_every: String,
    draw: bool,
}

impl Default for XsForm {
    fn default() -> Self {
        XsForm {
            name: String::new(),
            interval: String::new(),
            left: String::new(),
            right: String::new(),
            vscale: String::new(),
            columns: 3,
            grid: true,
            grid_every: String::new(),
            vgrid: true,
            vgrid_every: String::new(),
            draw: false,
        }
    }
}

/// The names of the centerlines in the drawing (the layers `CL-<name>`).
fn centerline_names(app: &CadApp) -> Vec<String> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    let mut v: Vec<String> =
        d.layers.iter().filter_map(|l| l.name.get(..3).filter(|p| p.eq_ignore_ascii_case("CL-")).map(|_| l.name[3..].to_string())).collect();
    v.sort();
    v
}

fn xs_params(f: &XsForm) -> Result<Value, String> {
    if f.name.is_empty() {
        return Err(crate::tl!("Choose a centerline, or draw one first.").to_string());
    }
    let mut m = Map::new();
    m.insert("name".into(), json!(f.name));
    for (key, text, field) in [
        ("interval", &f.interval, "Section interval"),
        ("leftWidth", &f.left, "Width left"),
        ("rightWidth", &f.right, "Width right"),
        ("vScale", &f.vscale, "Vertical exaggeration"),
    ] {
        put(&mut m, key, text, field, positive).map_err(bad_field)?;
    }
    m.insert("columns".into(), json!(f.columns));
    if f.grid {
        match positive(&f.grid_every).map_err(|()| bad_field("Horizontal line every"))? {
            Some(v) => m.insert("gridEvery".into(), json!(v)),
            None => m.insert("gridEvery".into(), json!(true)),
        };
    }
    if f.vgrid {
        match positive(&f.vgrid_every).map_err(|()| bad_field("Vertical line every"))? {
            Some(v) => m.insert("vGridEvery".into(), json!(v)),
            None => m.insert("vGridEvery".into(), json!(true)),
        };
    }
    Ok(Value::Object(m))
}

pub fn xsection(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let names = centerline_names(app);
    shell::<XsForm>(
        app,
        ctx,
        open,
        "xsection_dialog",
        crate::tl!("Cross Sections"),
        |ui, f| {
            if !names.contains(&f.name) {
                f.name = names.first().cloned().unwrap_or_default();
            }
            ui.label(crate::tl!("Centerline"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("xsection_centerline")
                    .selected_text(if f.name.is_empty() { crate::tl!("(none)").to_string() } else { f.name.clone() })
                    .show_ui(ui, |ui| {
                        for n in &names {
                            ui.selectable_value(&mut f.name, n.clone(), n);
                        }
                    });
                f.draw = ui.button(crate::tl!("Draw new...")).clicked();
            });
            ui.end_row();
            ui.label(crate::tl!("Section interval"));
            edit(ui, &mut f.interval, true);
            ui.end_row();
            ui.label(crate::tl!("Width left"));
            edit(ui, &mut f.left, true);
            ui.end_row();
            ui.label(crate::tl!("Width right"));
            edit(ui, &mut f.right, true);
            ui.end_row();
            ui.label(crate::tl!("Vertical exaggeration"));
            ui.add(egui::TextEdit::singleline(&mut f.vscale).hint_text("1").desired_width(140.0));
            ui.end_row();
            ui.label(crate::tl!("Sections per row"));
            ui.add(egui::DragValue::new(&mut f.columns).range(1..=50));
            ui.end_row();
            ui.label(crate::tl!("Horizontal lines"));
            ui.checkbox(&mut f.grid, "");
            ui.end_row();
            ui.label(crate::tl!("Horizontal line every"));
            ui.add_enabled_ui(f.grid, |ui| edit(ui, &mut f.grid_every, true));
            ui.end_row();
            ui.label(crate::tl!("Vertical lines (distance across)"));
            ui.checkbox(&mut f.vgrid, "");
            ui.end_row();
            ui.label(crate::tl!("Vertical line every"));
            ui.add_enabled_ui(f.vgrid, |ui| edit(ui, &mut f.vgrid_every, true));
            ui.end_row();
        },
        |app, f| {
            // Draw a centerline first: the dialog closes and the command asks for the points.
            if std::mem::take(&mut f.draw) {
                app.ui.dialog = None;
                app.start("alignment");
            }
        },
        |app, f| {
            let p = xs_params(f)?;
            app.run("xsection", p)?;
            let _ = app.session.execute("zoom.extents", &Value::Null);
            Ok(())
        },
    );
}

// ---- Horizontal Alignment ----

#[derive(Clone)]
struct AlignForm {
    /// The PIs this form belongs to (a new alignment starts a new form).
    pis: Vec<[f64; 2]>,
    name: String,
    radius: String,
    start: String,
    ticks: bool,
    ticks_every: String,
    text_height: String,
    labels: bool,
    /// A radius per interior PI; empty uses the default one.
    per_pi: Vec<String>,
}

impl Default for AlignForm {
    fn default() -> Self {
        AlignForm {
            pis: Vec::new(),
            name: String::new(),
            radius: "100".into(),
            start: String::new(),
            ticks: true,
            ticks_every: String::new(),
            text_height: String::new(),
            labels: true,
            per_pi: Vec::new(),
        }
    }
}

fn align_params(f: &AlignForm) -> Result<Value, String> {
    let mut m = Map::new();
    let radius = match f.radius.trim() {
        "" => 100.0,
        t => t
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| format!("{}: {}", crate::i18n::t("Curve radius"), crate::tl!("enter zero or a positive number")))?,
    };
    m.insert("radius".into(), json!(radius));
    let last = f.pis.len().saturating_sub(1);
    let mut pis = Vec::with_capacity(f.pis.len());
    for (i, q) in f.pis.iter().enumerate() {
        let own = if i == 0 || i == last { "" } else { f.per_pi.get(i).map(|t| t.trim()).unwrap_or("") };
        if own.is_empty() {
            pis.push(json!([q[0], q[1]]));
        } else {
            let r = own
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| format!("PI {i}: {}", crate::tl!("enter zero or a positive number")))?;
            pis.push(json!([q[0], q[1], r]));
        }
    }
    m.insert("pis".into(), Value::Array(pis));
    put_text(&mut m, "name", &f.name);
    put(&mut m, "startStation", &f.start, "Start station", number).map_err(bad_field)?;
    if f.ticks {
        match positive(&f.ticks_every).map_err(|()| bad_field("Mark every"))? {
            Some(v) => m.insert("ticks".into(), json!(v)),
            None => m.insert("ticks".into(), json!(true)),
        };
    }
    put(&mut m, "textHeight", &f.text_height, "Text height", positive).map_err(bad_field)?;
    m.insert("labels".into(), json!(f.labels));
    Ok(Value::Object(m))
}

/// The picture: a circular curve between two tangents with its elements, and next to it the PIs
/// the person chose.
fn align_picture(ui: &mut egui::Ui, pis: &[[f64; 2]]) {
    use egui::{Align2, Pos2, Stroke};
    let (resp, p) = ui.allocate_painter(vec2(340.0, 150.0), egui::Sense::hover());
    let r = resp.rect;
    let ink = ui.visuals().text_color();
    let accent = ui.visuals().hyperlink_color;
    let faint = ui.visuals().weak_text_color();
    let font = crate::theme::small();
    // The curve, in the left 200 points.
    let at = |x: f32, y: f32| Pos2::new(r.left() + x, r.top() + y);
    let phi = 40f32.to_radians();
    let t = 70.0f32;
    let pi = at(105.0, 14.0);
    let tc = pi + vec2(-phi.sin(), phi.cos()) * t;
    let ct = pi + vec2(phi.sin(), phi.cos()) * t;
    let half = 90f32.to_radians() - phi;
    let rad = t / half.tan();
    let o = pi + vec2(0.0, t.hypot(rad));
    let far_l = pi + vec2(-phi.sin(), phi.cos()) * t * 1.55;
    let far_r = pi + vec2(phi.sin(), phi.cos()) * t * 1.55;
    let dash = |a: Pos2, b: Pos2, w: f32, c: egui::Color32| {
        p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(w, c), 4.0, 3.0));
    };
    p.line_segment([far_l, tc], Stroke::new(1.8, accent));
    p.line_segment([ct, far_r], Stroke::new(1.8, accent));
    let arc: Vec<Pos2> = (0..=32)
        .map(|k| {
            let a = (-90.0 - half.to_degrees() + 2.0 * half.to_degrees() * k as f32 / 32.0).to_radians();
            o + vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    p.add(egui::Shape::line(arc, Stroke::new(2.2, accent)));
    dash(tc, pi, 1.0, faint);
    dash(pi, ct, 1.0, faint);
    dash(o, tc, 1.0, faint);
    dash(o, ct, 1.0, faint);
    dash(pi, o, 1.0, faint);
    for q in [pi, tc, ct, o] {
        p.circle_filled(q, 2.6, ink);
    }
    let lbl = |q: Pos2, a: Align2, s: &str| {
        p.text(q, a, s, font.clone(), ink);
    };
    lbl(pi + vec2(0.0, -2.0), Align2::CENTER_BOTTOM, "PI");
    lbl(tc + vec2(-4.0, 2.0), Align2::RIGHT_TOP, "TC");
    lbl(ct + vec2(4.0, 2.0), Align2::LEFT_TOP, "CT");
    lbl(o + vec2(5.0, 0.0), Align2::LEFT_CENTER, "O");
    lbl(tc.lerp(pi, 0.5) + vec2(-4.0, -2.0), Align2::RIGHT_BOTTOM, "T");
    lbl(ct.lerp(pi, 0.5) + vec2(4.0, -2.0), Align2::LEFT_BOTTOM, "T");
    lbl(o.lerp(tc, 0.5) + vec2(-3.0, 3.0), Align2::RIGHT_TOP, "R");
    lbl(pi.lerp(o, 0.5) + vec2(4.0, -8.0), Align2::LEFT_CENTER, "E");
    lbl(pi + vec2(0.0, 22.0), Align2::CENTER_TOP, "Δ");
    lbl(at(105.0, 14.0 + t.hypot(rad) - rad - 3.0), Align2::CENTER_BOTTOM, "L");
    // The PIs chosen.
    let box_ = egui::Rect::from_min_max(at(212.0, 4.0), at(336.0, 146.0));
    p.rect_stroke(box_, 3.0, Stroke::new(1.0, faint), egui::StrokeKind::Inside);
    if pis.len() >= 2 {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for q in pis {
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let inner = box_.shrink(10.0);
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
        let scale = (f64::from(inner.width().min(inner.height())) / span) as f32;
        let to = |q: &[f64; 2]| {
            Pos2::new(
                inner.center().x + ((q[0] - (lo[0] + hi[0]) / 2.0) as f32) * scale,
                inner.center().y - ((q[1] - (lo[1] + hi[1]) / 2.0) as f32) * scale,
            )
        };
        let line: Vec<Pos2> = pis.iter().take(2000).map(to).collect();
        p.add(egui::Shape::line(line.clone(), Stroke::new(1.4, accent)));
        for (i, q) in line.iter().enumerate() {
            p.circle_filled(*q, 2.4, ink);
            if pis.len() <= 12 {
                p.text(*q + vec2(3.0, -3.0), Align2::LEFT_BOTTOM, i.to_string(), font.clone(), faint);
            }
        }
    }
}

pub fn roadalign(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let pis = app.align_pis.clone();
    let names_used = centerline_names(app);
    shell::<AlignForm>(
        app,
        ctx,
        open,
        "roadalign_dialog",
        crate::tl!("Horizontal Alignment"),
        |ui, f| {
            if f.pis != pis {
                *f = AlignForm { pis: pis.clone(), per_pi: vec![String::new(); pis.len()], ..AlignForm::default() };
            }
            ui.label("");
            align_picture(ui, &pis);
            ui.end_row();
            ui.label(crate::tl!("Alignment name"));
            ui.add(egui::TextEdit::singleline(&mut f.name).hint_text(crate::tl!("Automatic")).desired_width(140.0));
            ui.end_row();
            if !f.name.trim().is_empty() && names_used.iter().any(|n| n.eq_ignore_ascii_case(f.name.trim())) {
                ui.label("");
                ui.colored_label(egui::Color32::LIGHT_RED, crate::tl!("A centerline with this name exists."));
                ui.end_row();
            }
            ui.label(crate::tl!("Curve radius"));
            ui.add(egui::TextEdit::singleline(&mut f.radius).hint_text("100").desired_width(140.0));
            ui.end_row();
            if pis.len() > 2 {
                ui.label(crate::tl!("Radius per PI"));
                ui.collapsing(crate::tl!("Different radius at a PI"), |ui| {
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        egui::Grid::new("roadalign_per_pi").num_columns(2).show(ui, |ui| {
                            for i in 1..pis.len() - 1 {
                                ui.label(format!("PI {i}"));
                                if let Some(t) = f.per_pi.get_mut(i) {
                                    ui.add(egui::TextEdit::singleline(t).hint_text(f.radius.trim()).desired_width(100.0));
                                }
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.end_row();
            }
            ui.label(crate::tl!("Start station"));
            ui.add(egui::TextEdit::singleline(&mut f.start).hint_text("0").desired_width(140.0));
            ui.end_row();
            ui.label(crate::tl!("Station marks"));
            ui.checkbox(&mut f.ticks, "");
            ui.end_row();
            ui.label(crate::tl!("Mark every"));
            ui.add_enabled_ui(f.ticks, |ui| edit(ui, &mut f.ticks_every, true));
            ui.end_row();
            ui.label(crate::tl!("Text height"));
            edit(ui, &mut f.text_height, true);
            ui.end_row();
            ui.label(crate::tl!("Curve data at the PIs"));
            ui.checkbox(&mut f.labels, "");
            ui.end_row();
        },
        |_, _| {},
        |app, f| {
            let p = align_params(f)?;
            app.run("roadalign", p)?;
            let _ = app.session.execute("zoom.extents", &Value::Null);
            Ok(())
        },
    );
}

/// Every frame: once ROADALIGN has chosen its PIs, open the settings dialog.
pub fn align_resume(app: &mut CadApp) {
    if app.session.running.is_some() {
        return;
    }
    if let Some(v) = app.session.align_pending.take() {
        app.align_pis = v.iter().map(|q| [q.x, q.y]).collect();
        app.ui.dialog = Some("roadalign".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alignment_picture_draws_with_any_pis() {
        let ctx = egui::Context::default();
        for pis in [vec![], vec![[0.0, 0.0]], vec![[0.0, 0.0], [0.0, 0.0]], vec![[0.0, 0.0], [1e9, -1e9], [5.0, 5.0]]] {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| align_picture(ui, &pis));
            out.textures_delta.clear();
        }
    }

    #[test]
    fn the_alignment_form_gives_the_pis_with_their_radii() {
        let f = AlignForm {
            pis: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]],
            per_pi: vec![String::new(), "60".into(), String::new()],
            name: "Road 1".into(),
            ..AlignForm::default()
        };
        let p = align_params(&f).unwrap_or_default();
        assert_eq!(p["pis"], json!([[0.0, 0.0], [100.0, 0.0, 60.0], [100.0, 100.0]]));
        assert_eq!(p["radius"], json!(100.0));
        assert_eq!(p["name"], json!("Road 1"));
        assert_eq!(p["ticks"], json!(true));
        let bad = AlignForm { radius: "-5".into(), ..f.clone() };
        assert!(align_params(&bad).is_err());
        let bad = AlignForm { per_pi: vec![String::new(), "abc".into(), String::new()], ..f };
        assert!(align_params(&bad).is_err());
    }

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
    fn sections_need_a_centerline_and_ask_for_the_horizontal_lines() {
        assert!(xs_params(&XsForm::default()).is_err(), "no centerline");
        let f = XsForm { name: "Line 1A".into(), ..XsForm::default() };
        let p = xs_params(&f).unwrap_or_default();
        assert_eq!((p["name"].as_str(), p["gridEvery"].clone(), p.get("interval")), (Some("Line 1A"), json!(true), None));
        let f = XsForm { name: "Line 1A".into(), grid_every: "2.5".into(), interval: "50".into(), ..XsForm::default() };
        let p = xs_params(&f).unwrap_or_default();
        assert_eq!((p["gridEvery"].clone(), p["interval"].clone()), (json!(2.5), json!(50.0)));
        assert_eq!(xs_params(&XsForm { name: "L".into(), ..XsForm::default() }).unwrap_or_default()["vGridEvery"], json!(true));
        let f = XsForm { name: "L".into(), vgrid_every: "10".into(), ..XsForm::default() };
        assert_eq!(xs_params(&f).unwrap_or_default()["vGridEvery"], json!(10.0));
        let f = XsForm { name: "L".into(), grid: false, vgrid: false, ..XsForm::default() };
        assert!(xs_params(&f).unwrap_or_default().get("gridEvery").is_none() && xs_params(&f).unwrap_or_default().get("vGridEvery").is_none());
        let f = XsForm { name: "L".into(), grid_every: "-1".into(), ..XsForm::default() };
        assert!(xs_params(&f).is_err());
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
