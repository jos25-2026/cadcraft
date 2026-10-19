//! The Contours dialog: the survey icon opens it; OK runs the `contour` command with the fields.

use egui::vec2;
use serde_json::{Map, Value, json};

use crate::CadApp;

#[derive(Clone)]
struct Form {
    interval: String,
    major_every: u32,
    smooth: u32,
    labels: bool,
    text_height: String,
    decimals: u32,
    prefix: String,
}

impl Default for Form {
    fn default() -> Self {
        Form { interval: String::new(), major_every: 5, smooth: 2, labels: true, text_height: String::new(), decimals: 1, prefix: String::new() }
    }
}

/// An empty field means "automatic"; anything else must be a positive number.
fn positive(text: &str) -> Result<Option<f64>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0).map(Some).ok_or(())
}

/// The `contour` parameters for a form, or the name of the invalid field.
fn params(f: &Form) -> Result<Value, &'static str> {
    let mut m = Map::new();
    if let Some(v) = positive(&f.interval).map_err(|()| "Contour interval")? {
        m.insert("interval".into(), json!(v));
    }
    if let Some(v) = positive(&f.text_height).map_err(|()| "Label text height")? {
        m.insert("textHeight".into(), json!(v));
    }
    m.insert("majorEvery".into(), json!(f.major_every));
    m.insert("smooth".into(), json!(f.smooth));
    m.insert("labels".into(), json!(f.labels));
    m.insert("decimals".into(), json!(f.decimals));
    if !f.prefix.trim().is_empty() {
        m.insert("layerPrefix".into(), json!(f.prefix.trim()));
    }
    Ok(Value::Object(m))
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("contour_dialog_form");
    let mut f = ctx.data_mut(|d| d.get_temp::<Form>(id)).unwrap_or_default();
    let err_id = egui::Id::new("contour_dialog_error");
    let mut err = ctx.data_mut(|d| d.get_temp::<String>(err_id)).unwrap_or_default();
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new(crate::tl!("Contours")).id(egui::Id::new("Contours")).open(open).resizable(false).collapsible(false).show(ctx, |ui| {
        egui::Grid::new("contour_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            ui.label(crate::tl!("Contour interval"));
            ui.add(egui::TextEdit::singleline(&mut f.interval).hint_text(crate::tl!("Automatic")).desired_width(120.0));
            ui.end_row();
            ui.label(crate::tl!("Major contour every"));
            ui.add(egui::DragValue::new(&mut f.major_every).range(1..=1000));
            ui.end_row();
            ui.label(crate::tl!("Smoothing (0-4)"));
            ui.add(egui::DragValue::new(&mut f.smooth).range(0..=4));
            ui.end_row();
            ui.label(crate::tl!("Elevation labels"));
            ui.checkbox(&mut f.labels, "");
            ui.end_row();
            ui.label(crate::tl!("Label text height"));
            ui.add_enabled(f.labels, egui::TextEdit::singleline(&mut f.text_height).hint_text(crate::tl!("Automatic")).desired_width(120.0));
            ui.end_row();
            ui.label(crate::tl!("Decimals"));
            ui.add(egui::DragValue::new(&mut f.decimals).range(0..=6));
            ui.end_row();
            ui.label(crate::tl!("Layer prefix"));
            ui.add(egui::TextEdit::singleline(&mut f.prefix).desired_width(120.0));
            ui.end_row();
        });
        if !err.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &err);
        }
        ui.horizontal(|ui| {
            ok = ui.button(crate::tl!("OK")).clicked();
            cancel = ui.button(crate::tl!("Cancel")).clicked();
        });
    });
    if ok {
        match params(&f) {
            Ok(p) => match app.run("contour", p) {
                Ok(_) => {
                    err.clear();
                    *open = false;
                }
                Err(e) => err = e,
            },
            Err(field) => err = format!("{}: {}", crate::i18n::t(field), crate::tl!("enter a positive number")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fields_are_automatic_and_bad_ones_are_named() {
        let p = params(&Form::default()).unwrap_or_default();
        assert!(p.get("interval").is_none() && p["majorEvery"] == json!(5));
        let f = Form { interval: "2.5".into(), ..Form::default() };
        assert_eq!(params(&f).unwrap_or_default()["interval"], json!(2.5));
        for bad in ["-1", "0", "abc", "NaN"] {
            let f = Form { interval: bad.into(), ..Form::default() };
            assert_eq!(params(&f), Err("Contour interval"), "{bad}");
        }
    }
}
