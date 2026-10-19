//! XSECTION: cross sections along a centerline. A section line is cut across the centerline at every
//! station (a chosen interval), reaching a chosen width to the left and right; the ground surface (the
//! TIN) is read along each line, and every section is drawn as a profile (offset across, elevation
//! up) on a sheet laid out to the right of the drawing, with the station lines drawn on the plan.

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, Handle, Line, LwPolyline};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::mapgrid::{default_extent, nice_spacing};
use super::terrain::{handles_param, layer_name, surface};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most stations, samples per section and samples in all.
const MAX_STATIONS: usize = 5000;
const MAX_SAMPLES: usize = 4000;
const MAX_TOTAL_SAMPLES: usize = 3_000_000;
/// Most vertices of a centerline.
const MAX_VERTS: usize = 100_000;
/// Largest coordinate or length accepted.
const MAX_COORD: f64 = 1.0e12;

/// The prefix of the layer a named centerline lives on: `CL-Line 1A`.
pub const CENTERLINE_PREFIX: &str = "CL-";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("alignment", "Survey Centerline...", run_centerline)
            .alias(&["surveycl", "drawalignment", "xscl"])
            .menu(&["Draw", "Survey Centerline..."])
            .params(
                "{points: [[x, y], ...], name?: \"Line 1A\" (default: the next free name Line 1A, Line 2A, ...)} → {name, layer, length}. The \
                 polyline is drawn on the layer `CL-<name>`; XSECTION lists the centerlines by name; not CENTERLINE, which draws a centre line between two lines",
            )
            .interactive(|_| Ok(Box::new(ClM::default()))),
        CommandSpec::new("xsection", "Cross Sections...", run_xsection)
            .alias(&["crosssections", "xs", "sections"])
            .menu(&["Tools", "Terrain", "Cross Sections..."])
            .params(
                "{centerline?: handle of a polyline or line, points?: [[x, y], ...] (the centerline when no handle), interval?: distance between \
                 sections (default: a round number giving about 10), width?: distance covered on each side (default: a tenth of the length), \
                 leftWidth?, rightWidth?, name?: a centerline made with ALIGNMENT (\"Line 1A\"), vGridEvery?: draw vertical lines every this much distance from the centerline, labelled under the section (true: a round number), gridEvery?: draw horizontal lines at every this much elevation (true: a round number), step?: spacing of the samples across (default: a hundredth of the width), vScale?: vertical \
                 exaggeration (default 1), columns?: sections per row of the sheet (default 3), at?: [x, y] top left of the sheet (default: \
                 right of the drawing), textHeight?, handles?: [3DFACE triangles] (default: the TIN layer, else a TIN is built from the points), \
                 plan?: true (draw the section lines), layerPrefix?: \"XS\"} → {stations, sections, datum}",
            )
            .interactive(|_| Ok(Box::new(XsM::default()))),
    ]
}

/// Triangles binned on a grid so a point finds its triangle without scanning all of them.
struct Index<'a> {
    tris: &'a [[Vec3; 3]],
    min: Vec2,
    cell: f64,
    n: usize,
    bins: Vec<Vec<u32>>,
}

impl<'a> Index<'a> {
    fn new(tris: &'a [[Vec3; 3]]) -> Option<Self> {
        let (mut lo, mut hi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for v in tris.iter().flatten() {
            lo = Vec2::new(lo.x.min(v.x), lo.y.min(v.y));
            hi = Vec2::new(hi.x.max(v.x), hi.y.max(v.y));
        }
        let span = (hi.x - lo.x).max(hi.y - lo.y);
        if !(span.is_finite() && span > 0.0) {
            return None;
        }
        let n = ((tris.len() as f64).sqrt() as usize).clamp(1, 512);
        let cell = span / n as f64;
        let mut bins: Vec<Vec<u32>> = vec![Vec::new(); n * n];
        let mut entries = 0usize;
        let at = |v: f64, m: f64| (((v - m) / cell).floor().max(0.0) as usize).min(n - 1);
        for (i, t) in tris.iter().enumerate() {
            let (x0, x1) = (t.iter().map(|v| v.x).fold(f64::INFINITY, f64::min), t.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max));
            let (y0, y1) = (t.iter().map(|v| v.y).fold(f64::INFINITY, f64::min), t.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max));
            for iy in at(y0, lo.y)..=at(y1, lo.y) {
                for ix in at(x0, lo.x)..=at(x1, lo.x) {
                    entries += 1;
                    if entries > 60_000_000 {
                        return None;
                    }
                    bins[iy * n + ix].push(i as u32);
                }
            }
        }
        Some(Index { tris, min: lo, cell, n, bins })
    }

    /// The surface height at (x, y); `None` outside the surface.
    fn z(&self, x: f64, y: f64) -> Option<f64> {
        let (fx, fy) = ((x - self.min.x) / self.cell, (y - self.min.y) / self.cell);
        if !(fx.is_finite() && fy.is_finite() && fx >= 0.0 && fy >= 0.0) {
            return None;
        }
        let (ix, iy) = ((fx as usize).min(self.n - 1), (fy as usize).min(self.n - 1));
        for &i in self.bins.get(iy * self.n + ix)? {
            let t = self.tris.get(i as usize)?;
            let det = (t[1].y - t[2].y) * (t[0].x - t[2].x) + (t[2].x - t[1].x) * (t[0].y - t[2].y);
            if det.abs() < 1e-18 {
                continue;
            }
            let l0 = ((t[1].y - t[2].y) * (x - t[2].x) + (t[2].x - t[1].x) * (y - t[2].y)) / det;
            let l1 = ((t[2].y - t[0].y) * (x - t[2].x) + (t[0].x - t[2].x) * (y - t[2].y)) / det;
            let l2 = 1.0 - l0 - l1;
            let e = -1e-9;
            if l0 >= e && l1 >= e && l2 >= e {
                return Some(l0 * t[0].z + l1 * t[1].z + l2 * t[2].z);
            }
        }
        None
    }
}

/// The centerline vertices of a line or polyline (curved segments run straight between vertices).
fn line_of(s: &Session, h: Handle) -> Option<Vec<Vec2>> {
    let d = s.doc().ok()?;
    let pts: Vec<Vec2> = match &d.entity(h)?.kind {
        EntityKind::Line(l) => vec![Vec2::new(l.a.x, l.a.y), Vec2::new(l.b.x, l.b.y)],
        EntityKind::LwPolyline(pl) => {
            let mut v: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
            if pl.closed
                && let Some(f) = v.first().copied()
            {
                v.push(f);
            }
            v
        }
        EntityKind::Polyline3d(pl) => pl.points.iter().map(|p| Vec2::new(p.x, p.y)).collect(),
        _ => return None,
    };
    (pts.len() <= MAX_VERTS).then_some(pts)
}

/// The layer of the centerline called `name`.
fn centerline_layer(cmd: &str, name: &str) -> Result<String> {
    let name = name.trim();
    super::terrain::layer_name(cmd, &format!("{CENTERLINE_PREFIX}{name}"))
        .map_err(|_| bad(cmd, "a centerline name is 1 to 60 characters without < > / \\ \" : ; ? * | = `"))
}

/// The vertices of the centerline called `name` (the first line or polyline on its layer).
fn named_line(s: &Session, name: &str) -> Result<Vec<Vec2>> {
    let layer = centerline_layer("xsection", name)?;
    let d = s.doc()?;
    d.model
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case(&layer))
        .find_map(|e| line_of(s, e.handle))
        .ok_or_else(|| bad("xsection", format!("no centerline named \"{}\": draw one with ALIGNMENT", name.trim())))
}

/// `Line 1A`, `Line 2A`, ...: the first name no layer uses yet.
fn next_name(s: &Session) -> String {
    let used = |n: &str| s.doc().is_ok_and(|d| d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&format!("{CENTERLINE_PREFIX}{n}"))));
    (1..=100_000).map(|i| format!("Line {i}A")).find(|n| !used(n)).unwrap_or_else(|| "Line".into())
}

/// ALIGNMENT: draws the points as a polyline named `name` (its own layer), to cut sections along.
fn run_centerline(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = centerline(s, &json!({ "points": p.get("points").cloned().unwrap_or(Value::Null) }))?;
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => next_name(s),
    };
    let layer = centerline_layer("alignment", &name)?;
    if s.doc()?.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer)) {
        return Err(bad("alignment", format!("a centerline named \"{name}\" exists: use another name")));
    }
    let length: f64 = pts.windows(2).map(|w| (w[1] - w[0]).len()).sum();
    let verts = pts.iter().map(|q| PolyVertex::new(*q)).collect();
    let kind = EntityKind::LwPolyline(LwPolyline { vertices: verts, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false });
    let (space, base) = (s.space(), s.current_common());
    let d = s.doc_mut()?;
    if d.ensure_layer(&layer)
        && let Some(l) = d.layers.iter_mut().find(|l| l.name == layer)
    {
        l.color = Color::Index(2);
    }
    d.add(&space, Common { layer: layer.clone(), color: Color::ByLayer, ..base }, kind)?;
    let msg = format!("centerline {name}: {} points, length {length:.2}", pts.len());
    s.echo(msg.clone());
    Ok(json!({ "name": name, "layer": layer, "length": length, "points": pts.len(), "message": msg }))
}

fn centerline(s: &Session, p: &Value) -> Result<Vec<Vec2>> {
    let raw: Vec<Vec2> = if let Some(a) = p.get("points").and_then(Value::as_array) {
        if a.len() > MAX_VERTS {
            return Err(bad("xsection", format!("more than {MAX_VERTS} centerline points")));
        }
        a.iter().map(|v| point_value(v).ok_or_else(|| bad("xsection", "`points` must be [x, y] pairs"))).collect::<Result<_>>()?
    } else if let Some(name) = str_param(p, "name") {
        named_line(s, name)?
    } else if let Some(t) = str_param(p, "centerline") {
        let h = Handle::parse_hex(t).ok_or_else(|| bad("xsection", "`centerline` is not a handle"))?;
        line_of(s, h).ok_or_else(|| bad("xsection", "`centerline` must be a line or polyline"))?
    } else {
        return Err(bad("xsection", "`name` (a centerline), `centerline` (a handle) or `points` is required"));
    };
    if raw.iter().any(|v| !(v.x.is_finite() && v.y.is_finite() && v.x.abs() <= MAX_COORD && v.y.abs() <= MAX_COORD)) {
        return Err(bad("xsection", "the centerline has invalid coordinates"));
    }
    let pts = distinct_points(&raw);
    if pts.len() < 2 {
        return Err(bad("xsection", "the centerline needs at least two different points"));
    }
    Ok(pts)
}

/// Station text: kilometres + metres, `1+250.00`.
fn station(s: f64) -> String {
    format!("{}+{:06.2}", (s / 1000.0).floor() as i64, s.rem_euclid(1000.0))
}

/// Position and unit direction of the centerline at distance `at` along it.
fn locate(line: &[Vec2], cum: &[f64], at: f64) -> (Vec2, Vec2) {
    let i = cum.partition_point(|c| *c <= at).saturating_sub(1).min(line.len().saturating_sub(2));
    let (a, b) = (line[i], line[i + 1]);
    let len = (b - a).len().max(f64::MIN_POSITIVE);
    let dir = (b - a) * (1.0 / len);
    let t = ((at - cum[i]) / len).clamp(0.0, 1.0);
    (a + (b - a) * t, dir)
}

fn run_xsection(s: &mut Session, p: &Value) -> Result<Value> {
    let line = centerline(s, p)?;
    let mut cum: Vec<f64> = vec![0.0];
    for w in line.windows(2) {
        cum.push(cum.last().copied().unwrap_or(0.0) + (w[1] - w[0]).len());
    }
    let total = cum.last().copied().unwrap_or(0.0);
    if !(total.is_finite() && total > 0.0 && total <= MAX_COORD) {
        return Err(bad("xsection", "the centerline has no usable length"));
    }
    let interval = size_param("xsection", p, "interval", false)?.unwrap_or_else(|| nice_spacing(total / 2.0));
    let width = size_param("xsection", p, "width", false)?.unwrap_or_else(|| nice_spacing(total / 2.0));
    let left = size_param("xsection", p, "leftWidth", false)?.unwrap_or(width);
    let right = size_param("xsection", p, "rightWidth", false)?.unwrap_or(width);
    let vscale = size_param("xsection", p, "vScale", false)?.unwrap_or(1.0);
    let span = left + right;
    let step = size_param("xsection", p, "step", false)?.unwrap_or(span / 100.0);
    let th = size_param("xsection", p, "textHeight", false)?.unwrap_or(span / 40.0);
    if ![interval, left, right, vscale, span, step, th].iter().all(|v| v.is_finite() && *v <= MAX_COORD) {
        return Err(bad("xsection", "sizes must be finite and not larger than 1e12"));
    }
    let columns = match p.get("columns") {
        None | Some(Value::Null) => 3,
        Some(v) => v
            .as_u64()
            .filter(|c| (1..=50).contains(c))
            .and_then(|c| usize::try_from(c).ok())
            .ok_or_else(|| bad("xsection", "`columns` must be 1 to 50"))?,
    };
    let prefix = layer_name("xsection", str_param(p, "layerPrefix").unwrap_or("XS"))?;
    let plan = bool_or(p, "plan", true);
    // Horizontal lines at every `gridEvery` of elevation (true: a round number).
    // Vertical lines at every `vGridEvery` of offset from the centerline (true: a round number).
    let vgrid: Option<Option<f64>> = match p.get("vGridEvery") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(None),
        Some(_) => Some(size_param("xsection", p, "vGridEvery", false)?),
    };
    let grid: Option<Option<f64>> = match p.get("gridEvery") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(None),
        Some(_) => Some(size_param("xsection", p, "gridEvery", false)?),
    };

    let stations_f = (total / interval).floor();
    if !(stations_f.is_finite() && stations_f < MAX_STATIONS as f64) {
        return Err(bad("xsection", format!("more than {MAX_STATIONS} sections: use a larger `interval`")));
    }
    let mut at: Vec<f64> = (0..=(stations_f as usize)).map(|i| i as f64 * interval).collect();
    if total - at.last().copied().unwrap_or(0.0) > total * 1e-9 {
        at.push(total);
    }
    let per = ((span / step).floor() + 1.0).max(2.0);
    if !(per.is_finite() && per <= MAX_SAMPLES as f64) || per as usize * at.len() > MAX_TOTAL_SAMPLES {
        return Err(bad("xsection", "too many samples: use a larger `step` or `interval`"));
    }
    let per = per as usize;

    let tris = surface(s, &handles_param(p), "TIN")?;
    let index = Index::new(&tris).ok_or_else(|| bad("xsection", "the surface is too detailed or empty"))?;

    // Sample every section: offset -left (left of the direction of travel) to +right.
    struct Sec {
        sta: f64,
        pos: Vec2,
        normal: Vec2,
        z: Vec<Option<f64>>,
        z0: Option<f64>,
    }
    let mut secs: Vec<Sec> = Vec::with_capacity(at.len());
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for &sta in &at {
        let (pos, dir) = locate(&line, &cum, sta);
        let normal = Vec2::new(-dir.y, dir.x);
        let z: Vec<Option<f64>> = (0..per)
            .map(|k| {
                let off = -left + span * k as f64 / (per - 1) as f64;
                let q = pos - normal * off;
                index.z(q.x, q.y)
            })
            .collect();
        for v in z.iter().flatten() {
            zmin = zmin.min(*v);
            zmax = zmax.max(*v);
        }
        let z0 = index.z(pos.x, pos.y);
        secs.push(Sec { sta, pos, normal, z, z0 });
    }
    if !(zmin.is_finite() && zmax.is_finite()) {
        return Err(bad("xsection", "the centerline does not cross the surface"));
    }

    // The sheet: a common datum below the lowest ground, equal cells, rows going down.
    let range = (zmax - zmin).max(1e-9);
    let vstep = nice_spacing(range);
    let datum = ((zmin - 0.1 * range) / vstep).floor() * vstep;
    let grid_step = grid.map(|g| g.unwrap_or(vstep));
    let vgrid_step = vgrid.map(|g| g.unwrap_or_else(|| nice_spacing(span)));
    if let Some(g) = vgrid_step
        && !(0.0..300.0).contains(&(span / g))
    {
        return Err(bad("xsection", "too many vertical lines: use a larger `vGridEvery`"));
    }
    if let Some(g) = grid_step
        && !(0.0..300.0).contains(&((zmax - datum) / g))
    {
        return Err(bad("xsection", "too many horizontal lines: use a larger `gridEvery`"));
    }
    let pad = th * 5.0;
    let (cw, ch) = (span + 2.0 * pad, (zmax - datum) * vscale + 2.0 * pad);
    let gap = th * 3.0;
    let ext = default_extent(s).ok();
    let origin = match p.get("at") {
        None | Some(Value::Null) => ext.map_or(Vec2::ZERO, |e| Vec2::new(e.max.x + cw * 0.5, e.max.y)),
        Some(v) => point_value(v).ok_or_else(|| bad("xsection", "`at` must be [x, y]"))?,
    };
    if ![cw, ch, gap, origin.x, origin.y].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) {
        return Err(bad("xsection", "the sheet would be too large"));
    }

    let (l_lines, l_ground, l_frame, l_text, l_cl) =
        (format!("{prefix}-LINES"), format!("{prefix}-GROUND"), format!("{prefix}-FRAME"), format!("{prefix}-TEXT"), format!("{prefix}-CL"));
    let l_grid = format!("{prefix}-GRID");
    let v3 = |q: Vec2| Vec3::new(q.x, q.y, 0.0);
    let seg = |a: Vec2, b: Vec2| EntityKind::Line(Line { a: v3(a), b: v3(b) });
    let polyline = |pts: Vec<Vec2>, closed: bool| {
        EntityKind::LwPolyline(LwPolyline {
            vertices: pts.into_iter().map(PolyVertex::new).collect(),
            closed,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    };
    let text = |q: Vec2, t: &str, h: f64| make_text(s, q, t, h, 0.0, (cadcraft_doc::HAlign::Left, cadcraft_doc::VAlign::Baseline));
    let text_below = |q: Vec2, t: &str, h: f64| make_text(s, q, t, h, 0.0, (cadcraft_doc::HAlign::Center, cadcraft_doc::VAlign::Top));
    let mut items: Vec<(&str, EntityKind)> = Vec::new();
    for (k, sec) in secs.iter().enumerate() {
        let (col, row) = (k % columns, k / columns);
        let top = Vec2::new(origin.x + col as f64 * (cw + gap), origin.y - row as f64 * (ch + gap));
        let (x0, base) = (top.x + pad, top.y - ch + pad);
        let (xl, xr) = (x0, x0 + span);
        let label = format!("STA {}", station(sec.sta));
        if plan {
            items.push((&l_lines, seg(sec.pos + sec.normal * left, sec.pos - sec.normal * right)));
            items.push((&l_text, text(sec.pos + sec.normal * left, &label, th)));
        }
        items.push((
            &l_frame,
            polyline(vec![top, Vec2::new(top.x + cw, top.y), Vec2::new(top.x + cw, top.y - ch), Vec2::new(top.x, top.y - ch)], true),
        ));
        items.push((&l_frame, seg(Vec2::new(xl, base), Vec2::new(xr, base))));
        if let Some(g) = vgrid_step {
            // Offsets from the centerline: lines up to the top of the ground, distances under the baseline.
            let decimals = if g.fract() == 0.0 { 0 } else { 1 };
            let top_y = base + (zmax - datum) * vscale;
            let (kl, kr) = ((left / g).floor() as i64, (right / g).floor() as i64);
            for k in -kl..=kr {
                let x = x0 + left + k as f64 * g;
                if k != 0 {
                    items.push((&l_grid, seg(Vec2::new(x, base), Vec2::new(x, top_y))));
                }
                items.push((&l_text, text_below(Vec2::new(x, base - th * 0.4), &format!("{:.decimals$}", (k as f64 * g).abs()), th)));
            }
        }
        if let Some(g) = grid_step {
            let decimals = if g.fract() == 0.0 { 0 } else { 2 };
            for k in 1..=(((zmax - datum) / g).floor() as usize) {
                let level = datum + k as f64 * g;
                let y = base + (level - datum) * vscale;
                items.push((&l_grid, seg(Vec2::new(xl, y), Vec2::new(xr, y))));
                items.push((&l_text, text(Vec2::new(top.x + th * 0.5, y - th * 0.3), &format!("{level:.decimals$}"), th)));
            }
        }
        items.push((&l_cl, seg(Vec2::new(x0 + left, base), Vec2::new(x0 + left, top.y - pad * 0.5))));
        items.push((&l_text, text(Vec2::new(top.x + pad, top.y - pad * 0.6), &label, th * 1.4)));
        items.push((&l_text, text(Vec2::new(top.x + th * 0.5, base - th * 2.8), &format!("Datum {datum:.2}"), th)));
        items.push((&l_text, text(Vec2::new(xl, base - th * 4.4), &format!("L {left:.1}"), th)));
        items.push((&l_text, text(Vec2::new(xr - th * 6.0, base - th * 4.4), &format!("R {right:.1}"), th)));
        let mut run: Vec<Vec2> = Vec::new();
        for (j, z) in sec.z.iter().enumerate() {
            match z {
                Some(z) => run.push(Vec2::new(xl + span * j as f64 / (per - 1) as f64, base + (z - datum) * vscale)),
                None => {
                    if run.len() >= 2 {
                        items.push((&l_ground, polyline(std::mem::take(&mut run), false)));
                    }
                    run.clear();
                }
            }
        }
        if run.len() >= 2 {
            items.push((&l_ground, polyline(run, false)));
        }
        if let Some(z0) = sec.z0 {
            items.push((&l_text, text(Vec2::new(x0 + left + th * 0.4, base + (z0 - datum) * vscale + th * 0.6), &format!("{z0:.2}"), th)));
        }
    }

    let space = s.space();
    let base_common = s.current_common();
    let n_items = items.len();
    {
        let d = s.doc_mut()?;
        for (l, c) in [(&l_lines, 1u8), (&l_ground, 3), (&l_frame, 8), (&l_text, 7), (&l_cl, 2), (&l_grid, 8)] {
            if d.ensure_layer(l)
                && let Some(x) = d.layers.iter_mut().find(|x| x.name == *l)
            {
                x.color = Color::Index(c);
            }
        }
        for (layer, kind) in items {
            let com = Common { layer: layer.to_string(), color: Color::ByLayer, ..base_common.clone() };
            d.add(&space, com, kind)?;
        }
    }
    let msg = format!(
        "{} cross sections every {interval} along {total:.2}, {left}/{right} each side, sheet at ({:.2}, {:.2}), datum {datum:.2}",
        secs.len(),
        origin.x,
        origin.y
    );
    s.echo(msg.clone());
    Ok(json!({
        "sections": secs.len(), "stations": at, "datum": datum, "vScale": vscale, "zMin": zmin, "zMax": zmax,
        "sheetAt": [origin.x, origin.y], "cell": [cw, ch], "objects": n_items, "message": msg,
    }))
}

/// XSECTION at the command line: pick a polyline or line, or click the centerline points, then
/// the interval, the width each side and the vertical exaggeration.
#[derive(Default)]
struct XsM {
    pts: Vec<Vec2>,
    /// 0 centerline, 1 interval, 2 width, 3 vertical exaggeration.
    step: u8,
    interval: Option<f64>,
    width: Option<f64>,
}

impl XsM {
    fn length(&self) -> f64 {
        self.pts.windows(2).map(|w| (w[1] - w[0]).len()).sum()
    }
}

fn positive(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

impl Interactive for XsM {
    fn name(&self) -> &'static str {
        "XSECTION"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        surface(s, &[], "TIN")?;
        if let Some(pts) = s.selection().into_iter().find_map(|h| line_of(s, h)).map(|v| distinct_points(&v)).filter(|v| v.len() >= 2) {
            self.pts = pts;
            self.step = 1;
            s.set_selection(Vec::new());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.step {
            0 if self.pts.is_empty() => Prompt::new("Select the centerline (a polyline or line), or click its first point", Accept::SELECT),
            0 => {
                let mut p = Prompt::new("Next centerline point (Enter to finish)", Accept::POINT);
                p.base = self.pts.last().copied();
                p
            }
            1 => {
                let mut p = Prompt::new("Distance between cross sections", Accept::TEXT);
                p.default = Some(nice_spacing(self.length() / 2.0).to_string());
                p
            }
            2 => {
                let mut p = Prompt::new("Width to cover on each side of the centerline", Accept::TEXT);
                p.default = Some(nice_spacing(self.length() / 2.0).to_string());
                p
            }
            _ => {
                let mut p = Prompt::new("Vertical exaggeration", Accept::TEXT);
                p.default = Some("1".into());
                p
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.step == 0 {
            match i {
                Input::Pick(hs) if self.pts.is_empty() => {
                    match hs.into_iter().find_map(|h| line_of(s, h)).map(|v| distinct_points(&v)).filter(|v| v.len() >= 2) {
                        Some(pts) => {
                            self.pts = pts;
                            self.step = 1;
                        }
                        None => s.echo("Pick a line or a polyline.".to_string()),
                    }
                }
                Input::Point(q) => self.pts.push(q),
                Input::Enter if self.pts.len() >= 2 => {
                    self.pts = distinct_points(&self.pts);
                    if self.pts.len() < 2 {
                        s.echo("The centerline needs at least two different points.".to_string());
                        self.pts.clear();
                        return Ok(Step::Continue);
                    }
                    // The clicked line stays in the drawing as a named centerline.
                    let pts: Vec<[f64; 2]> = self.pts.iter().map(|q| [q.x, q.y]).collect();
                    run_centerline(s, &json!({ "points": pts }))?;
                    self.step = 1;
                }
                Input::Enter => {
                    s.echo("Click at least two points, or pick a polyline.".to_string());
                    self.pts.clear();
                }
                _ => s.echo("Select the centerline, or click its points.".to_string()),
            }
            return Ok(Step::Continue);
        }
        let text = match i {
            Input::Text(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => return Ok(Step::Cancel),
        };
        match self.step {
            1 | 2 => {
                if !text.is_empty() {
                    match positive(&text) {
                        Some(v) if self.step == 1 => self.interval = Some(v),
                        Some(v) => self.width = Some(v),
                        None => {
                            s.echo("Enter a positive number.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                self.step += 1;
                Ok(Step::Continue)
            }
            _ => {
                let v = if text.is_empty() { Some(1.0) } else { positive(&text) };
                let Some(v) = v else {
                    s.echo("Enter a positive number.".to_string());
                    return Ok(Step::Continue);
                };
                let pts: Vec<[f64; 2]> = self.pts.iter().map(|q| [q.x, q.y]).collect();
                let mut params = json!({ "points": pts, "vScale": v });
                if let Some(x) = self.interval {
                    params["interval"] = json!(x);
                }
                if let Some(x) = self.width {
                    params["width"] = json!(x);
                }
                run_xsection(s, &params)?;
                Ok(Step::Done)
            }
        }
    }
}

/// ALIGNMENT at the command line: click the points, then the name.
#[derive(Default)]
struct ClM {
    pts: Vec<Vec2>,
    naming: bool,
}

impl Interactive for ClM {
    fn name(&self) -> &'static str {
        "ALIGNMENT"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if self.naming {
            let mut p = Prompt::new("Name of the centerline", Accept::TEXT);
            p.default = Some(next_name(s));
            p
        } else if self.pts.is_empty() {
            Prompt::new("Specify the first point of the centerline", Accept::POINT)
        } else {
            let mut p = Prompt::new("Next point (Enter to finish)", Accept::POINT);
            p.base = self.pts.last().copied();
            p
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Cancel => Ok(Step::Cancel),
            Input::Point(q) if !self.naming => {
                self.pts.push(q);
                Ok(Step::Continue)
            }
            Input::Enter if !self.naming => {
                self.pts = distinct_points(&self.pts);
                if self.pts.len() < 2 {
                    s.echo("The centerline needs at least two different points.".to_string());
                    return Ok(Step::Continue);
                }
                self.naming = true;
                Ok(Step::Continue)
            }
            Input::Text(t) if self.naming => self.finish(s, t.trim()),
            Input::Enter if self.naming => self.finish(s, ""),
            _ => Ok(Step::Continue),
        }
    }
}

impl ClM {
    fn finish(&mut self, s: &mut Session, name: &str) -> Result<Step> {
        let pts: Vec<[f64; 2]> = self.pts.iter().map(|q| [q.x, q.y]).collect();
        match run_centerline(s, &json!({ "points": pts, "name": name })) {
            Ok(_) => Ok(Step::Done),
            Err(e) => {
                s.echo(e.to_string());
                Ok(Step::Continue)
            }
        }
    }
}
