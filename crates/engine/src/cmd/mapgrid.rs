//! Map sheet furniture for an ortho photo or a plan: MAPGRID draws a coordinate grid (full lines
//! or crosses) with a frame and the easting and northing labels, and NORTHARROW inserts a north
//! arrow. Both draw ordinary lines, polylines, solids and text on their own layers.

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, HAlign, Handle, Line, LwPolyline, Solid, VAlign, entity_bounds};
use cadcraft_geom::{Bounds2, PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most grid lines per direction (a spacing far too small for the area would flood the drawing).
const MAX_LINES: usize = 1000;
/// Largest coordinate magnitude accepted.
const MAX_COORD: f64 = 1.0e12;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mapgrid", "Map Grid...", run_grid)
            .alias(&["gridlines", "coordgrid"])
            .menu(&["Insert", "Map Grid..."])
            .params(
                "{handles?: [handle of an image or rectangle to frame], extent?: [minX, minY, maxX, maxY] (default: the attached images, else the whole drawing), spacing?: grid interval \
                 (default: a round number giving about 5 cells), style?: \"lines\" | \"cross\" (default lines), textHeight?, \
                 decimals?: 0, crossSize?: arm length of a cross, margin?: width of the label band, frame?: true, \
                 labelPrefix?: true (E / N before the numbers), thousands?: false, layerPrefix?: \"GRID\"}",
            )
            .interactive(|_| Ok(Box::new(GridM::default()))),
        CommandSpec::new("northarrow", "North Arrow...", run_north)
            .alias(&["northsymbol"])
            .menu(&["Insert", "North Arrow..."])
            .params("{at?: [x, y] (default: the top right of the images or drawing), size?: diameter, rotation?: degrees counter-clockwise, layer?: \"NORTH\"}")
            .interactive(|_| Ok(Box::new(NorthM::default()))),
    ]
}

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

fn finite(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

/// The extent to grid: the attached images, or the whole drawing.
fn default_extent(s: &Session) -> Result<Bounds2> {
    let d = s.doc()?;
    let mut images = Bounds2::EMPTY;
    let mut all = Bounds2::EMPTY;
    for e in d.model.iter() {
        if let EntityKind::Image(i) = &e.kind {
            let (w, h) = (i.u.xy() * i.size.x, i.v.xy() * i.size.y);
            let o = i.insert.xy();
            for c in [o, o + w, o + w + h, o + h] {
                images.add(c);
            }
        }
        let b = entity_bounds(d, e, 0);
        if !b.is_empty() {
            all = all.union(&b);
        }
    }
    let b = if images.is_empty() { all } else { images };
    if b.is_empty() || !finite(b.min.x) || !finite(b.min.y) || !finite(b.max.x) || !finite(b.max.y) {
        return Err(bad("mapgrid", "nothing to grid: attach an image or draw something first, or pass `extent`"));
    }
    Ok(b)
}

/// The bounds of the given objects: an image's frame (turned or not) or a rectangle's outline.
fn extent_of(s: &Session, handles: &[Handle]) -> Result<Bounds2> {
    let d = s.doc()?;
    let mut b = Bounds2::EMPTY;
    for h in handles {
        let Some(e) = d.entity(*h) else { continue };
        if let EntityKind::Image(i) = &e.kind {
            let (w, hh) = (i.u.xy() * i.size.x, i.v.xy() * i.size.y);
            let o = i.insert.xy();
            for c in [o, o + w, o + w + hh, o + hh] {
                b.add(c);
            }
        } else {
            let eb = entity_bounds(d, e, 0);
            if !eb.is_empty() {
                b = b.union(&eb);
            }
        }
    }
    if b.is_empty() || !(finite(b.min.x) && finite(b.min.y) && finite(b.max.x) && finite(b.max.y)) {
        return Err(bad("mapgrid", "the chosen objects have no extent: pick an image or a rectangle"));
    }
    Ok(b)
}

fn extent_param(p: &Value) -> Result<Option<Bounds2>> {
    match p.get("extent") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let [x0, y0, x1, y1] = a[..] else { return Err(bad("mapgrid", "`extent` must be [minX, minY, maxX, maxY]")) };
            if ![x0, y0, x1, y1].iter().all(|v| finite(*v)) {
                return Err(bad("mapgrid", "`extent` must be finite numbers"));
            }
            Ok(Some(Bounds2::new(Vec2::new(x0, y0), Vec2::new(x1, y1))))
        }
    }
}

/// A round interval (1, 2 or 5 × a power of ten) that divides `span` into about five cells.
fn nice_spacing(span: f64) -> f64 {
    let raw = (span / 5.0).max(f64::MIN_POSITIVE);
    let pow = 10f64.powf(raw.log10().floor());
    let m = raw / pow;
    let step = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    step * pow
}

/// Multiples of `step` strictly inside `(lo, hi)`.
fn positions(lo: f64, hi: f64, step: f64) -> Result<Vec<f64>> {
    let eps = step * 1e-9;
    let first = ((lo + eps) / step).floor() as i64 + 1;
    let mut out = Vec::new();
    let mut k = first;
    loop {
        let v = k as f64 * step;
        if v >= hi - eps {
            break;
        }
        if v > lo + eps {
            out.push(v);
        }
        if out.len() > MAX_LINES {
            return Err(bad("mapgrid", format!("more than {MAX_LINES} grid lines: use a larger `spacing`")));
        }
        k += 1;
    }
    Ok(out)
}

fn label(prefix: &str, v: f64, decimals: usize, thousands: bool) -> String {
    let mut t = format!("{v:.decimals$}");
    if thousands {
        let (int, frac) = match t.split_once('.') {
            Some((i, f)) => (i.to_string(), format!(".{f}")),
            None => (t.clone(), String::new()),
        };
        let (sign, digits) = int.strip_prefix('-').map(|d| ("-", d)).unwrap_or(("", int.as_str()));
        let mut grouped = String::new();
        for (n, c) in digits.chars().enumerate() {
            if n > 0 && (digits.len() - n) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        t = format!("{sign}{grouped}{frac}");
    }
    format!("{prefix}{t}")
}

fn layer_prefix(p: &Value) -> Result<String> {
    let raw = str_param(p, "layerPrefix").unwrap_or("GRID").trim();
    if raw.is_empty() || raw.chars().count() > 64 || raw.chars().any(|c| c.is_control() || "<>/\\\":;?*|=`".contains(c)) {
        return Err(bad("mapgrid", "`layerPrefix` must be 1 to 64 characters without < > / \\ \" : ; ? * | = `"));
    }
    Ok(raw.to_string())
}

fn run_grid(s: &mut Session, p: &Value) -> Result<Value> {
    let handles: Vec<Handle> = p
        .get("handles")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().and_then(Handle::parse_hex)).collect())
        .unwrap_or_default();
    let ext = match (extent_param(p)?, handles.is_empty()) {
        (Some(e), _) => e,
        (None, false) => extent_of(s, &handles)?,
        (None, true) => default_extent(s)?,
    };
    let (w, h) = (ext.width(), ext.height());
    if !(w > 0.0 && h > 0.0) {
        return Err(bad("mapgrid", "the extent has no area"));
    }
    let spacing = size_param("mapgrid", p, "spacing", false)?.unwrap_or_else(|| nice_spacing(w.max(h)));
    let text_h = size_param("mapgrid", p, "textHeight", false)?.unwrap_or(spacing / 12.0);
    let cross_size = size_param("mapgrid", p, "crossSize", false)?.unwrap_or(text_h * 2.0);
    let margin = size_param("mapgrid", p, "margin", false)?.unwrap_or(text_h * 3.2);
    let decimals = match p.get("decimals") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_u64().filter(|d| *d <= 6).and_then(|d| usize::try_from(d).ok()).ok_or_else(|| bad("mapgrid", "`decimals` must be 0 to 6"))?,
    };
    let style = str_param(p, "style").unwrap_or("lines").to_ascii_lowercase();
    let cross = match style.as_str() {
        "lines" | "line" | "solid" => false,
        "cross" | "crosses" | "plus" => true,
        _ => return Err(bad("mapgrid", "`style` must be \"lines\" or \"cross\"")),
    };
    let (frame, prefixed, thousands) = (bool_or(p, "frame", true), bool_or(p, "labelPrefix", true), bool_or(p, "thousands", false));
    let prefix = layer_prefix(p)?;
    if !(finite(spacing) && finite(text_h) && finite(cross_size) && finite(margin)) {
        return Err(bad("mapgrid", "sizes must be finite"));
    }

    let xs = positions(ext.min.x, ext.max.x, spacing)?;
    let ys = positions(ext.min.y, ext.max.y, spacing)?;
    let (l_grid, l_frame, l_text) = (format!("{prefix}-LINES"), format!("{prefix}-FRAME"), format!("{prefix}-TEXT"));
    let space = s.space();
    let base = s.current_common();
    {
        let d = s.doc_mut()?;
        for l in [&l_grid, &l_frame, &l_text] {
            d.ensure_layer(l);
        }
    }
    let on = |layer: &str, color: u8| Common { layer: layer.to_string(), color: Color::Index(color), ..base.clone() };
    let mut count = 0usize;
    let mut add = |s: &mut Session, c: Common, k: EntityKind| -> Result<()> {
        s.doc_mut()?.add(&space, c, k)?;
        count += 1;
        Ok(())
    };
    let seg = |a: Vec2, b: Vec2| EntityKind::Line(Line { a: v3(a), b: v3(b) });
    let rect = |r: Bounds2| {
        EntityKind::LwPolyline(LwPolyline {
            vertices: [r.min, Vec2::new(r.max.x, r.min.y), r.max, Vec2::new(r.min.x, r.max.y)].into_iter().map(PolyVertex::new).collect(),
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    };

    // Grid: full lines, or crosses at the intersections with ticks on the frame.
    let gc = on(&l_grid, 8);
    if cross {
        let arm = cross_size.min(spacing / 2.0);
        for x in &xs {
            for y in &ys {
                add(s, gc.clone(), seg(Vec2::new(x - arm, *y), Vec2::new(x + arm, *y)))?;
                add(s, gc.clone(), seg(Vec2::new(*x, y - arm), Vec2::new(*x, y + arm)))?;
            }
        }
        for x in &xs {
            add(s, gc.clone(), seg(Vec2::new(*x, ext.min.y), Vec2::new(*x, ext.min.y + arm)))?;
            add(s, gc.clone(), seg(Vec2::new(*x, ext.max.y), Vec2::new(*x, ext.max.y - arm)))?;
        }
        for y in &ys {
            add(s, gc.clone(), seg(Vec2::new(ext.min.x, *y), Vec2::new(ext.min.x + arm, *y)))?;
            add(s, gc.clone(), seg(Vec2::new(ext.max.x, *y), Vec2::new(ext.max.x - arm, *y)))?;
        }
    } else {
        for x in &xs {
            add(s, gc.clone(), seg(Vec2::new(*x, ext.min.y), Vec2::new(*x, ext.max.y)))?;
        }
        for y in &ys {
            add(s, gc.clone(), seg(Vec2::new(ext.min.x, *y), Vec2::new(ext.max.x, *y)))?;
        }
    }

    // Frame: the neat line at the extent and an outer border around the label band.
    if frame {
        let fc = on(&l_frame, 7);
        add(s, fc.clone(), rect(ext))?;
        add(s, fc, rect(ext.expand(margin)))?;
    }

    // Labels: eastings above and below, northings left and right (reading upward).
    let tc = on(&l_text, 3);
    let (e_pre, n_pre) = if prefixed { ("E ", "N ") } else { ("", "") };
    let gap = text_h * 0.4;
    let mut labels = 0usize;
    // A label that would run past the end of its side into the corner (where the other side's
    // labels are) is left out; its grid line still shows.
    let fits = |t: &str, at: f64, lo: f64, hi: f64| {
        let half = t.chars().count() as f64 * text_h * 0.5;
        at - half >= lo && at + half <= hi
    };
    for x in &xs {
        let t = label(e_pre, *x, decimals, thousands);
        if !fits(&t, *x, ext.min.x, ext.max.x) {
            continue;
        }
        add(s, tc.clone(), make_text(s, Vec2::new(*x, ext.min.y - gap), &t, text_h, 0.0, (HAlign::Center, VAlign::Top)))?;
        add(s, tc.clone(), make_text(s, Vec2::new(*x, ext.max.y + gap), &t, text_h, 0.0, (HAlign::Center, VAlign::Bottom)))?;
        labels += 2;
    }
    let up = std::f64::consts::FRAC_PI_2;
    for y in &ys {
        let t = label(n_pre, *y, decimals, thousands);
        if !fits(&t, *y, ext.min.y, ext.max.y) {
            continue;
        }
        add(s, tc.clone(), make_text(s, Vec2::new(ext.min.x - gap, *y), &t, text_h, up, (HAlign::Center, VAlign::Bottom)))?;
        add(s, tc.clone(), make_text(s, Vec2::new(ext.max.x + gap, *y), &t, text_h, up, (HAlign::Center, VAlign::Top)))?;
        labels += 2;
    }

    let msg = format!("map grid: {} × {} lines every {spacing}, {labels} labels", xs.len(), ys.len());
    s.echo(msg.clone());
    Ok(json!({
        "extent": [ext.min.x, ext.min.y, ext.max.x, ext.max.y],
        "spacing": spacing,
        "style": if cross { "cross" } else { "lines" },
        "textHeight": text_h,
        "columns": xs.len(),
        "rows": ys.len(),
        "objects": count,
        "layers": [l_grid, l_frame, l_text],
        "message": msg,
    }))
}

fn run_north(s: &mut Session, p: &Value) -> Result<Value> {
    let ext = default_extent(s).ok();
    let size = size_param("northarrow", p, "size", false)?
        .unwrap_or_else(|| ext.map(|e| (e.width().min(e.height()) * 0.08).max(f64::MIN_POSITIVE)).unwrap_or(10.0));
    let c = match p.get("at") {
        None | Some(Value::Null) => match ext {
            Some(e) => Vec2::new(e.max.x - size * 1.0, e.max.y - size * 1.2),
            None => Vec2::ZERO,
        },
        Some(_) => point_req("northarrow", p, "at")?,
    };
    let rot = f64_or(p, "rotation", 0.0).to_radians();
    if !finite(size) || !finite(c.x) || !finite(c.y) || !rot.is_finite() {
        return Err(bad("northarrow", "`at`, `size` and `rotation` must be finite numbers"));
    }
    let layer = str_param(p, "layer").unwrap_or("NORTH").trim().to_string();
    if layer.is_empty() || layer.chars().count() > 64 || layer.chars().any(|c| c.is_control() || "<>/\\\":;?*|=`".contains(c)) {
        return Err(bad("northarrow", "`layer` must be 1 to 64 characters without < > / \\ \" : ; ? * | = `"));
    }
    let space = s.space();
    let base = s.current_common();
    s.doc_mut()?.ensure_layer(&layer);
    let com = Common { layer, color: Color::Index(7), ..base };

    let r = size / 2.0;
    // Points are given upright (north = +y) about the centre, then turned by the rotation.
    let at = |x: f64, y: f64| c + Vec2::new(x * r, y * r).rotate(rot);
    let tip = at(0.0, 0.92);
    let left = at(-0.38, -0.72);
    let right = at(0.38, -0.72);
    let notch = at(0.0, -0.36);
    let solid = EntityKind::Solid(Solid { corners: [v3(tip), v3(left), v3(notch), v3(notch)] });
    let outline = EntityKind::LwPolyline(LwPolyline {
        vertices: [tip, right, notch, left].into_iter().map(PolyVertex::new).collect(),
        closed: true,
        const_width: 0.0,
        elevation: 0.0,
        plinegen: false,
    });
    let ring = EntityKind::Circle(cadcraft_doc::Circle { center: v3(c), radius: r });
    let letter = make_text(s, at(0.0, 1.12), "N", r * 0.55, rot, (HAlign::Center, VAlign::Bottom));
    let d = s.doc_mut()?;
    for k in [ring, outline, solid, letter] {
        d.add(&space, com.clone(), k)?;
    }
    s.echo("north arrow inserted".to_string());
    Ok(json!({ "at": [c.x, c.y], "size": size, "rotation": rot.to_degrees(), "message": "north arrow inserted" }))
}

/// MAPGRID at the command line: the image or rectangle to frame, spacing, style, text size.
#[derive(Default)]
struct GridM {
    spacing: Option<f64>,
    cross: bool,
    /// 0 pick the object, 1 spacing, 2 style, 3 text height.
    step: u8,
    ext: Option<Bounds2>,
    handles: Vec<Handle>,
}

fn number(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

impl Interactive for GridM {
    fn name(&self) -> &'static str {
        "MAPGRID"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        // Objects already selected (pickfirst) frame the grid straight away.
        let sel = s.selection();
        if !sel.is_empty()
            && let Ok(b) = extent_of(s, &sel)
        {
            self.ext = Some(b);
            self.handles = sel;
            self.step = 1;
            s.set_selection(Vec::new());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let auto = self.ext.map(|e| nice_spacing(e.width().max(e.height())));
        match self.step {
            0 => Prompt::new("Select the image or rectangle that frames the grid (Enter: all images, else the whole drawing)", Accept::SELECT),
            1 => {
                let mut p = Prompt::new("Grid spacing in drawing units", Accept::TEXT);
                p.default = auto.map(|v| v.to_string());
                p
            }
            2 => {
                let mut p = Prompt::new("Grid style: Lines (full lines) or Cross", Accept::TEXT);
                p.default = Some("Lines".into());
                p
            }
            _ => {
                let mut p = Prompt::new("Text height of the coordinates", Accept::TEXT);
                p.default = self.spacing.or(auto).map(|v| (v / 12.0).to_string());
                p
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.step == 0 {
            match i {
                Input::Pick(hs) => match extent_of(s, &hs) {
                    Ok(b) => {
                        self.ext = Some(b);
                        self.handles = hs;
                        s.set_selection(Vec::new());
                    }
                    Err(e) => {
                        s.echo(e.to_string());
                        return Ok(Step::Continue);
                    }
                },
                Input::Enter => self.ext = Some(default_extent(s)?),
                Input::Cancel => return Ok(Step::Cancel),
                _ => {
                    s.echo("Click an image or a rectangle, or press Enter.".to_string());
                    return Ok(Step::Continue);
                }
            }
            self.step = 1;
            return Ok(Step::Continue);
        }
        let text = match i {
            Input::Text(t) => t.trim().to_string(),
            Input::Keyword(k) => k,
            Input::Enter => String::new(),
            _ => return Ok(Step::Cancel),
        };
        match self.step {
            1 => {
                if !text.is_empty() {
                    match number(&text) {
                        Some(v) => self.spacing = Some(v),
                        None => {
                            s.echo("Enter a positive number.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                self.step = 2;
                Ok(Step::Continue)
            }
            2 => {
                let t = text.to_ascii_lowercase();
                self.cross = t.starts_with('c') || t.starts_with('x') || t.starts_with('+');
                self.step = 3;
                Ok(Step::Continue)
            }
            _ => {
                let mut params = json!({ "style": if self.cross { "cross" } else { "lines" } });
                if let Some(e) = self.ext {
                    params["extent"] = json!([e.min.x, e.min.y, e.max.x, e.max.y]);
                }
                if let Some(v) = self.spacing {
                    params["spacing"] = json!(v);
                }
                if !text.is_empty() {
                    match number(&text) {
                        Some(v) => params["textHeight"] = json!(v),
                        None => {
                            s.echo("Enter a positive number.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                run_grid(s, &params)?;
                Ok(Step::Done)
            }
        }
    }
}

/// NORTHARROW at the command line: position, size, rotation.
#[derive(Default)]
struct NorthM {
    at: Option<Vec2>,
    size: Option<f64>,
}

impl Interactive for NorthM {
    fn name(&self) -> &'static str {
        "NORTHARROW"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.at, self.size) {
            (None, _) => Prompt::new("Specify the north arrow position", Accept::POINT),
            (Some(_), None) => Prompt::new("Diameter of the north arrow in drawing units (Enter for automatic)", Accept::TEXT),
            _ => {
                let mut p = Prompt::new("Rotation in degrees counter-clockwise (0 points up)", Accept::TEXT);
                p.default = Some("0".into());
                p
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.at, self.size, i) {
            (None, _, Input::Point(p)) => {
                self.at = Some(p);
                Ok(Step::Continue)
            }
            (Some(_), None, Input::Text(t)) if number(&t).is_some() => {
                self.size = number(&t);
                Ok(Step::Continue)
            }
            (Some(_), None, Input::Enter) => {
                self.size = Some(0.0);
                Ok(Step::Continue)
            }
            (Some(at), Some(size), i) => {
                let rot = match i {
                    Input::Text(t) => t.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0),
                    _ => 0.0,
                };
                let mut params = json!({ "at": [at.x, at.y], "rotation": rot });
                if size > 0.0 {
                    params["size"] = json!(size);
                }
                run_north(s, &params)?;
                Ok(Step::Done)
            }
            (_, _, Input::Cancel) => Ok(Step::Cancel),
            _ => {
                s.echo("Enter a positive number.".to_string());
                Ok(Step::Continue)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_a_round_number() {
        assert_eq!(nice_spacing(110.0), 20.0);
        assert_eq!(nice_spacing(1000.0), 200.0);
        assert_eq!(nice_spacing(7.0), 1.0);
    }

    #[test]
    fn positions_are_multiples_inside_the_range() {
        assert_eq!(positions(499_995.0, 500_105.0, 50.0).unwrap(), vec![500_000.0, 500_050.0, 500_100.0]);
        assert!(positions(0.0, 1.0e9, 1.0).is_err());
    }

    #[test]
    fn labels_group_thousands() {
        assert_eq!(label("E ", 1_520_000.0, 0, true), "E 1,520,000");
        assert_eq!(label("N ", -1234.5, 1, true), "N -1,234.5");
        assert_eq!(label("", 500_050.0, 0, false), "500050");
    }
}
