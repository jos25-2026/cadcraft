//! ROADPROFILE and ROADSECTION: the vertical design of a road along a named centerline.
//!
//! ROADPROFILE reads the ground along the centerline (the TIN), takes the design grade given as
//! PVIs (station, elevation, vertical-curve length; a symmetric parabola of that length is fitted
//! at each interior PVI) and draws the profile sheet: ground, design grade, grid, grades and the
//! curve data. The PVIs are kept in the drawing as text on the layer `PVI-<name>`.
//!
//! ROADSECTION cuts a section at every station, builds the road template on the design grade
//! (lanes with a crossfall, shoulders), runs cut or fill side slopes out to the ground, and gives
//! the cut and fill area of each section and the volumes by average end area.

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, HAlign, Hatch, HatchLoop, Line, LwPolyline, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::mapgrid::{default_extent, nice_spacing};
use super::terrain::{handles_param, layer_name, surface};
use super::xsection::{CENTERLINE_PREFIX, Index, centerline, locate, station};
use super::*;
use crate::{Result, Session};

const MAX_PVIS: usize = 1000;
const MAX_SAMPLES: usize = 20_000;
const MAX_STATIONS: usize = 3000;
const MAX_COORD: f64 = 1.0e12;
/// The layer prefix of the stored PVIs: `PVI-Line 1A`.
pub const PVI_PREFIX: &str = "PVI-";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("roadprofile", "Profile and Vertical Curves...", run_profile)
            .alias(&["profile", "verticalcurves", "vcurve", "gradeline"])
            .menu(&["Tools", "Road", "Profile and Vertical Curves..."])
            .params(
                "{name?: a centerline (ALIGNMENT or ROADALIGN) | points?: [[x, y], ...], pvis?: [[station, elevation, curveLength?], ...] (the design grade; \
                 omitted: the PVIs kept for `name`, else only the ground is drawn), startStation?: 0, step?: ground sampling distance, vScale?: vertical \
                 exaggeration (default 10), stationEvery?: vertical line spacing, gridEvery?: horizontal line spacing, at?: [x, y] top left of the sheet, textHeight?, \
                 handles?: [3DFACE], layerPrefix?: \"PR\"} → {length, ground: {min, max}, pvis, grades, curves: [{pvi, pvc, pvt, g1, g2, length, k, kind, ...}]}",
            ),
        CommandSpec::new("roadsection", "Road Cross Sections...", run_roadsection)
            .alias(&["roadxs", "cutfillsections", "roadsections"])
            .menu(&["Tools", "Road", "Road Cross Sections..."])
            .params(
                "{name: a centerline, pvis?: [[station, elevation, curveLength?], ...] (default: the PVIs kept by ROADPROFILE), startStation?: 0, interval?: 20, \
                 lanes?: lanes each side (1), laneWidth?: 3.5, shoulder?: 1.5, crossfall?: % (2), shoulderFall?: % (4), cutSlope?: horizontal per 1 vertical (1.5), \
                 fillSlope?: (2), reach?: farthest side-slope run from the shoulder (60), vScale?: 1, columns?: 3, at?, textHeight?, handles?, layerPrefix?: \"RS\", \
                 superelevation?: [[station, leftPercent, rightPercent], ...] (the slope of the lanes on each side, rising outwards from the crown is positive; \
                 -2, -2 is the normal crown, 6, -6 banks the road to fall to the right; linear between stations, the normal crown outside them), \
                 ditchDepth?: depth of a roadside ditch below the shoulder edge on cut sides (0, none), ditchWidth?: its flat bottom (0.6), \
                 ditchSlope?: its inner side slope, horizontal per 1 vertical (3), shade?: hatch the cut and fill areas (true)} \
                 → {sections: [{station, ground, design, cut, fill, daylightLeft, daylightRight, slopeLeft, slopeRight}], cutVolume, fillVolume}",
            ),
    ]
}

// ---- The design grade ----

#[derive(Clone, Copy, Debug)]
pub(super) struct Pvi {
    pub sta: f64,
    pub z: f64,
    pub len: f64,
}

/// A vertical curve of the grade, for the report.
#[derive(Clone, Copy, Debug)]
pub(super) struct VCurve {
    pub pvi: Pvi,
    pub g1: f64,
    pub g2: f64,
}

pub(super) struct Grade {
    pub pvis: Vec<Pvi>,
}

impl Grade {
    /// Validate: 2 or more PVIs in increasing station order, curves that fit between neighbours.
    pub(super) fn new(mut v: Vec<Pvi>) -> Result<Grade> {
        if v.len() < 2 || v.len() > MAX_PVIS {
            return Err(bad("roadprofile", format!("the design grade needs 2 to {MAX_PVIS} PVIs")));
        }
        if v.iter().any(|p| {
            !(p.sta.is_finite()
                && p.z.is_finite()
                && p.len.is_finite()
                && p.sta.abs() <= MAX_COORD
                && p.z.abs() <= MAX_COORD
                && (0.0..=MAX_COORD).contains(&p.len))
        }) {
            return Err(bad("roadprofile", "PVIs are [station, elevation, curve length (zero or more)] numbers"));
        }
        v.sort_by(|a, b| a.sta.total_cmp(&b.sta));
        let n = v.len();
        for (i, p) in v.iter_mut().enumerate() {
            if i == 0 || i + 1 == n {
                p.len = 0.0; // the ends have no curve
            }
        }
        for (a, b) in v.iter().zip(v.iter().skip(1)) {
            if b.sta - a.sta < 1e-6 {
                return Err(bad("roadprofile", format!("two PVIs at station {:.3}", a.sta)));
            }
            if a.len / 2.0 + b.len / 2.0 > b.sta - a.sta + 1e-9 {
                return Err(bad("roadprofile", format!("the vertical curves at stations {:.2} and {:.2} overlap: shorten them", a.sta, b.sta)));
            }
        }
        Ok(Grade { pvis: v })
    }

    pub(super) fn first(&self) -> f64 {
        self.pvis.first().map_or(0.0, |p| p.sta)
    }
    pub(super) fn last(&self) -> f64 {
        self.pvis.last().map_or(0.0, |p| p.sta)
    }

    /// The tangent grade (rise per run) between PVI `i` and `i + 1`.
    fn grade(&self, i: usize) -> f64 {
        match (self.pvis.get(i), self.pvis.get(i + 1)) {
            (Some(a), Some(b)) => (b.z - a.z) / (b.sta - a.sta),
            _ => 0.0,
        }
    }

    /// The design elevation at a station; `None` outside the first and last PVI.
    pub(super) fn z(&self, sta: f64) -> Option<f64> {
        if !(sta.is_finite() && sta >= self.first() - 1e-9 && sta <= self.last() + 1e-9) {
            return None;
        }
        let n = self.pvis.len();
        let i = self.pvis.partition_point(|p| p.sta <= sta).saturating_sub(1).min(n.checked_sub(2)?);
        let on_curve = |k: usize| -> Option<f64> {
            let p = self.pvis.get(k)?;
            if k == 0 || p.len <= 0.0 {
                return None;
            }
            let start = p.sta - p.len / 2.0;
            let x = sta - start;
            if !(0.0..=p.len).contains(&x) {
                return None;
            }
            let (g1, g2) = (self.grade(k - 1), self.grade(k));
            Some(p.z - g1 * p.len / 2.0 + g1 * x + (g2 - g1) / (2.0 * p.len) * x * x)
        };
        if let Some(z) = on_curve(i + 1).or_else(|| on_curve(i)) {
            return Some(z);
        }
        let p = self.pvis.get(i)?;
        Some(p.z + self.grade(i) * (sta - p.sta))
    }

    pub(super) fn curves(&self) -> Vec<VCurve> {
        let n = self.pvis.len();
        (1..n.saturating_sub(1)).filter_map(|k| Some(VCurve { pvi: *self.pvis.get(k)?, g1: self.grade(k - 1), g2: self.grade(k) })).collect()
    }

    /// Stations where the grade changes shape: PVIs and the ends of the curves.
    fn key_stations(&self) -> Vec<f64> {
        let mut v = Vec::new();
        for p in &self.pvis {
            v.push(p.sta);
            if p.len > 0.0 {
                v.push(p.sta - p.len / 2.0);
                v.push(p.sta + p.len / 2.0);
            }
        }
        v
    }
}

fn parse_pvis(v: &Value) -> Result<Vec<Pvi>> {
    let a = v.as_array().ok_or_else(|| bad("roadprofile", "`pvis` must be [[station, elevation, curveLength?], ...]"))?;
    if a.len() > MAX_PVIS {
        return Err(bad("roadprofile", format!("more than {MAX_PVIS} PVIs")));
    }
    a.iter()
        .map(|r| {
            let r = r.as_array()?;
            let len = match r.get(2) {
                None | Some(Value::Null) => 0.0,
                Some(x) => x.as_f64()?,
            };
            Some(Pvi { sta: r.first()?.as_f64()?, z: r.get(1)?.as_f64()?, len })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad("roadprofile", "`pvis` must be [[station, elevation, curveLength?], ...] numbers"))
}

/// The PVIs kept in the drawing for the centerline `name` (text `PVI sta,elev,len` on `PVI-<name>`).
pub(super) fn stored_pvis(s: &Session, name: &str) -> Vec<Pvi> {
    let Ok(d) = s.doc() else { return Vec::new() };
    let layer = format!("{PVI_PREFIX}{}", name.trim());
    let mut v: Vec<Pvi> = d
        .model
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case(&layer))
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => {
                let body = t.value.trim().strip_prefix("PVI")?.trim();
                let mut it = body.split(',').map(|x| x.trim().parse::<f64>().ok());
                Some(Pvi { sta: it.next()??, z: it.next()??, len: it.next().flatten().unwrap_or(0.0) })
            }
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.sta.total_cmp(&b.sta));
    v
}

/// `pvis` from the parameters, else those kept for `name`.
fn grade_for(s: &Session, p: &Value, cmd: &str) -> Result<Option<Grade>> {
    if let Some(v) = p.get("pvis").filter(|v| !v.is_null()) {
        return Grade::new(parse_pvis(v)?).map(Some);
    }
    if let Some(n) = str_param(p, "name") {
        let kept = stored_pvis(s, n);
        if kept.len() >= 2 {
            return Grade::new(kept).map(Some);
        }
    }
    let _ = cmd;
    Ok(None)
}

// ---- Shared helpers ----

fn cumulative(line: &[Vec2]) -> Vec<f64> {
    let mut cum: Vec<f64> = vec![0.0];
    for w in line.windows(2) {
        if let [a, b] = w {
            cum.push(cum.last().copied().unwrap_or(0.0) + (*b - *a).len());
        }
    }
    cum
}

fn v3(q: Vec2) -> Vec3 {
    Vec3::new(q.x, q.y, 0.0)
}
fn seg(a: Vec2, b: Vec2) -> EntityKind {
    EntityKind::Line(Line { a: v3(a), b: v3(b) })
}
fn polyline(pts: Vec<Vec2>, closed: bool) -> EntityKind {
    EntityKind::LwPolyline(LwPolyline {
        vertices: pts.into_iter().map(PolyVertex::new).collect(),
        closed,
        const_width: 0.0,
        elevation: 0.0,
        plinegen: false,
    })
}
/// A closed hatch over `verts`, filled with a standard line pattern.
fn hatch_region(verts: Vec<PolyVertex>, pattern: &str, angle: f64, scale: f64) -> EntityKind {
    EntityKind::Hatch(Hatch {
        pattern: pattern.to_string(),
        solid: false,
        loops: vec![HatchLoop { vertices: verts, outer: true, sources: Vec::new() }],
        scale,
        angle,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::default(),
        background: None,
        pattern_lines: Vec::new(),
    })
}
fn layer_colors(d: &mut cadcraft_doc::Drawing, layers: &[(&str, u8)]) {
    for (l, c) in layers {
        if d.ensure_layer(l)
            && let Some(x) = d.layers.iter_mut().find(|x| x.name == *l)
        {
            x.color = Color::Index(*c);
        }
    }
}
fn txt(s: &Session, q: Vec2, t: &str, h: f64) -> EntityKind {
    make_text(s, q, t, h, 0.0, (HAlign::Left, VAlign::Baseline))
}
fn txt_below(s: &Session, q: Vec2, t: &str, h: f64) -> EntityKind {
    make_text(s, q, t, h, 0.0, (HAlign::Center, VAlign::Top))
}

fn sheet_origin(s: &Session, p: &Value, cmd: &str, cw: f64) -> Result<Vec2> {
    let ext = default_extent(s).ok();
    let o = match p.get("at") {
        None | Some(Value::Null) => ext.map_or(Vec2::ZERO, |e| Vec2::new(e.max.x + cw * 0.5, e.max.y)),
        Some(v) => point_value(v).ok_or_else(|| bad(cmd, "`at` must be [x, y]"))?,
    };
    if !(o.x.is_finite() && o.y.is_finite() && o.x.abs() <= MAX_COORD && o.y.abs() <= MAX_COORD) {
        return Err(bad(cmd, "the sheet would be too large"));
    }
    Ok(o)
}

// ---- ROADPROFILE ----

fn run_profile(s: &mut Session, p: &Value) -> Result<Value> {
    let line = centerline(s, p)?;
    let cum = cumulative(&line);
    let total = cum.last().copied().unwrap_or(0.0);
    if !(total.is_finite() && total > 0.0 && total <= MAX_COORD) {
        return Err(bad("roadprofile", "the centerline has no usable length"));
    }
    let start = f64_or(p, "startStation", 0.0);
    if start.abs() > MAX_COORD {
        return Err(bad("roadprofile", "`startStation` is too large"));
    }
    let step = size_param("roadprofile", p, "step", false)?.unwrap_or(total / 200.0);
    let vscale = size_param("roadprofile", p, "vScale", false)?.unwrap_or(10.0);
    let th = size_param("roadprofile", p, "textHeight", false)?.unwrap_or(total / 150.0);
    if ![step, vscale, th].iter().all(|v| v.is_finite() && *v <= MAX_COORD) {
        return Err(bad("roadprofile", "sizes must be finite and not larger than 1e12"));
    }
    let nsamples = (total / step).floor();
    if !(nsamples.is_finite() && nsamples < MAX_SAMPLES as f64) {
        return Err(bad("roadprofile", "too many samples: use a larger `step`"));
    }
    let grade = grade_for(s, p, "roadprofile")?;
    let prefix = layer_name("roadprofile", str_param(p, "layerPrefix").unwrap_or("PR"))?;

    // Ground along the centerline.
    let tris = surface(s, &handles_param(p), "TIN")?;
    let index = Index::new(&tris).ok_or_else(|| bad("roadprofile", "the surface is too detailed or empty"))?;
    let mut stas: Vec<f64> = (0..=(nsamples as usize)).map(|i| i as f64 * step).collect();
    if total - stas.last().copied().unwrap_or(0.0) > total * 1e-9 {
        stas.push(total);
    }
    let ground: Vec<(f64, Option<f64>)> = stas
        .iter()
        .map(|d| {
            let (q, _) = locate(&line, &cum, *d);
            (start + d, index.z(q.x, q.y))
        })
        .collect();
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for z in ground.iter().filter_map(|g| g.1) {
        zmin = zmin.min(z);
        zmax = zmax.max(z);
    }
    if let Some(g) = &grade {
        for p in &g.pvis {
            zmin = zmin.min(p.z);
            zmax = zmax.max(p.z);
        }
    }
    if !(zmin.is_finite() && zmax.is_finite()) {
        return Err(bad("roadprofile", "the centerline does not cross the surface"));
    }
    if let Some(g) = &grade
        && (g.first() > start + total + 1e-9 || g.last() < start - 1e-9)
    {
        return Err(bad("roadprofile", "the PVIs are outside the stations of the centerline (check `startStation`)"));
    }

    // The sheet.
    let range = (zmax - zmin).max(1e-9);
    let vstep = size_param("roadprofile", p, "gridEvery", false)?.unwrap_or_else(|| nice_spacing(range));
    let hstep = size_param("roadprofile", p, "stationEvery", false)?.unwrap_or_else(|| nice_spacing(total));
    if !(0.0..300.0).contains(&(range / vstep)) || !(0.0..400.0).contains(&(total / hstep)) {
        return Err(bad("roadprofile", "too many grid lines: use larger `gridEvery` or `stationEvery`"));
    }
    let datum = ((zmin - 0.1 * range) / vstep).floor() * vstep;
    let pad = th * 6.0;
    let (cw, ch) = (total + 2.0 * pad, (zmax - datum) * vscale + 2.0 * pad);
    let origin = sheet_origin(s, p, "roadprofile", cw)?;
    if !(cw.is_finite() && ch.is_finite() && ch.abs() <= MAX_COORD) {
        return Err(bad("roadprofile", "the sheet would be too large"));
    }
    let (x0, base) = (origin.x + pad, origin.y - ch + pad);
    let at = |sta: f64, z: f64| Vec2::new(x0 + (sta - start), base + (z - datum) * vscale);
    let (l_ground, l_design, l_grid, l_text, l_frame, l_pvi) = (
        format!("{prefix}-GROUND"),
        format!("{prefix}-DESIGN"),
        format!("{prefix}-GRID"),
        format!("{prefix}-TEXT"),
        format!("{prefix}-FRAME"),
        format!("{prefix}-PVI"),
    );
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty());
    let l_store = name.map(|n| format!("{PVI_PREFIX}{n}"));
    if let Some(l) = &l_store {
        layer_name("roadprofile", l)?;
    }

    let mut items: Vec<(&str, EntityKind)> = Vec::new();
    items.push((
        &l_frame,
        polyline(vec![origin, Vec2::new(origin.x + cw, origin.y), Vec2::new(origin.x + cw, origin.y - ch), Vec2::new(origin.x, origin.y - ch)], true),
    ));
    // Elevation lines and station lines.
    let top_y = base + (zmax - datum) * vscale;
    let ndec = if vstep.fract() == 0.0 { 0 } else { 2 };
    for k in 0..=(((zmax - datum) / vstep).floor() as usize) {
        let level = datum + k as f64 * vstep;
        let y = base + (level - datum) * vscale;
        items.push((&l_grid, seg(Vec2::new(x0, y), Vec2::new(x0 + total, y))));
        items.push((&l_text, txt(s, Vec2::new(origin.x + th * 0.5, y - th * 0.3), &format!("{level:.ndec$}"), th)));
    }
    let first_line = (start / hstep).ceil() * hstep;
    for k in 0..=((total / hstep).floor() as usize + 1) {
        let sta = first_line + k as f64 * hstep;
        if sta > start + total + 1e-9 {
            break;
        }
        let x = x0 + (sta - start);
        items.push((&l_grid, seg(Vec2::new(x, base), Vec2::new(x, top_y))));
        items.push((&l_text, txt_below(s, Vec2::new(x, base - th * 0.4), &station(sta), th)));
    }
    // Ground profile (broken where the surface ends).
    let mut run: Vec<Vec2> = Vec::new();
    for (sta, z) in &ground {
        match z {
            Some(z) => run.push(at(*sta, *z)),
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

    let mut report_curves = Vec::new();
    let mut report_grades = Vec::new();
    let mut report_pvis = Vec::new();
    if let Some(g) = &grade {
        // Design grade: every sample and every key station inside the sheet.
        let mut st: Vec<f64> =
            ground.iter().map(|x| x.0).chain(g.key_stations()).filter(|x| *x >= start - 1e-9 && *x <= start + total + 1e-9).collect();
        st.sort_by(|a, b| a.total_cmp(b));
        st.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let pts: Vec<Vec2> = st.iter().filter_map(|x| g.z(*x).map(|z| at(*x, z))).collect();
        if pts.len() >= 2 {
            items.push((&l_design, polyline(pts, false)));
        }
        for (i, pv) in g.pvis.iter().enumerate() {
            let q = at(pv.sta, pv.z);
            items.push((&l_pvi, EntityKind::Circle(cadcraft_doc::Circle { center: v3(q), radius: th * 0.4 })));
            items.push((&l_text, txt(s, q + Vec2::new(th * 0.5, th * 0.8), &format!("PVI {} EL {:.2}", station(pv.sta), pv.z), th)));
            if pv.len > 0.0 {
                items.push((&l_text, txt(s, q + Vec2::new(th * 0.5, -th * 1.8), &format!("VC {:.1}", pv.len), th)));
            }
            report_pvis.push(json!({"station": pv.sta, "elevation": pv.z, "curveLength": pv.len}));
            if let (Some(a), Some(b)) = (g.pvis.get(i), g.pvis.get(i + 1)) {
                let gr = (b.z - a.z) / (b.sta - a.sta) * 100.0;
                let mid = at((a.sta + b.sta) / 2.0, (a.z + b.z) / 2.0);
                items.push((&l_text, txt(s, mid + Vec2::new(-th * 2.0, th * 1.6), &format!("{gr:+.2}%"), th)));
                report_grades.push(json!({"from": a.sta, "to": b.sta, "percent": gr}));
            }
        }
        for c in g.curves().iter().filter(|c| c.pvi.len > 0.0) {
            let (l, a_pct) = (c.pvi.len, (c.g2 - c.g1) * 100.0);
            let (pvc, pvt) = (c.pvi.sta - l / 2.0, c.pvi.sta + l / 2.0);
            let k = if a_pct.abs() > 1e-12 { l / a_pct.abs() } else { f64::INFINITY };
            let kind = if c.g2 < c.g1 { "crest" } else { "sag" };
            // The high or low point of the curve, where the grade is zero.
            let turning = (c.g2 - c.g1).abs().gt(&1e-12).then(|| -c.g1 * l / (c.g2 - c.g1)).filter(|x| (0.0..=l).contains(x)).map(|x| pvc + x);
            let turning_z = turning.and_then(|t| g.z(t));
            let ev = c.pvi.z + (c.g2 - c.g1) * l / 8.0;
            let q = at(c.pvi.sta, c.pvi.z);
            items.push((
                &l_text,
                txt(s, q + Vec2::new(th * 0.5, th * 3.2), &format!("{kind} A={:.2}% K={:.1}", a_pct.abs(), if k.is_finite() { k } else { 0.0 }), th),
            ));
            report_curves.push(json!({
                "pvi": c.pvi.sta, "pviElevation": c.pvi.z, "pvc": pvc, "pvcElevation": g.z(pvc), "pvt": pvt, "pvtElevation": g.z(pvt),
                "g1": c.g1 * 100.0, "g2": c.g2 * 100.0, "a": a_pct.abs(), "length": l, "k": if k.is_finite() { json!(k) } else { Value::Null },
                "kind": kind, "externalOffset": c.pvi.z - ev, "midElevation": ev, "turning": turning, "turningElevation": turning_z,
            }));
        }
    }
    items.push((&l_text, txt(s, Vec2::new(origin.x + pad, origin.y - pad * 0.6), &format!("PROFILE {}", name.unwrap_or("")), th * 1.4)));
    items.push((
        &l_text,
        txt(s, Vec2::new(origin.x + th * 0.5, base - th * 2.8), &format!("Datum {datum:.2}   H 1:1   V 1:{}", format_ratio(vscale)), th),
    ));

    let space = s.space();
    let base_common = s.current_common();
    let n_items = items.len();
    {
        let d = s.doc_mut()?;
        layer_colors(d, &[(&l_ground, 3), (&l_design, 1), (&l_grid, 8), (&l_text, 7), (&l_frame, 8), (&l_pvi, 2)]);
        if let (Some(l), Some(_)) = (&l_store, &grade) {
            // The PVIs are kept as text so ROADSECTION (and the next ROADPROFILE) find them again.
            let old: Vec<_> = d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(l)).map(|e| e.handle).collect();
            for h in old {
                d.remove_entity(h);
            }
            layer_colors(d, &[(l, 2)]);
        }
        for (layer, kind) in items {
            let com = Common { layer: layer.to_string(), color: Color::ByLayer, ..base_common.clone() };
            d.add(&space, com, kind)?;
        }
    }
    if let (Some(l), Some(g)) = (&l_store, &grade) {
        let kept: Vec<EntityKind> = g
            .pvis
            .iter()
            .map(|pv| txt(s, at(pv.sta, pv.z) + Vec2::new(0.0, -th * 4.0), &format!("PVI {:.3},{:.3},{:.3}", pv.sta, pv.z, pv.len), th * 0.7))
            .collect();
        let d = s.doc_mut()?;
        for kind in kept {
            let com = Common { layer: l.clone(), color: Color::ByLayer, ..base_common.clone() };
            d.add(&space, com, kind)?;
        }
    }
    let msg = format!(
        "Profile of {total:.2}: ground {zmin:.2} to {zmax:.2}{}, sheet at ({:.2}, {:.2})",
        grade.as_ref().map_or(String::new(), |g| format!(", {} PVIs, {} vertical curves", g.pvis.len(), report_curves.len())),
        origin.x,
        origin.y
    );
    s.echo(msg.clone());
    Ok(json!({
        "length": total, "ground": {"min": zmin, "max": zmax}, "datum": datum, "pvis": report_pvis, "grades": report_grades,
        "curves": report_curves, "sheetAt": [origin.x, origin.y], "objects": n_items, "message": msg,
    }))
}

fn format_ratio(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.2}") }
}

// ---- ROADSECTION ----

#[derive(Clone, Copy)]
struct Template {
    /// Half the paved width, then half the width with the shoulders.
    lanes_half: f64,
    half: f64,
    crossfall: f64,
    shoulder_fall: f64,
    cut_slope: f64,
    fill_slope: f64,
    /// A roadside ditch on the cut sides: depth below the shoulder edge (0: none), bottom width,
    /// inner side slope (horizontal per 1 vertical).
    ditch_depth: f64,
    ditch_width: f64,
    ditch_slope: f64,
    /// The outward slope (rise per run) of the lanes and of the shoulders: left, right.
    lane: [f64; 2],
    shoulder: [f64; 2],
}

/// The side of an offset: 0 left (negative offsets), 1 right.
fn side_of(u: f64) -> usize {
    usize::from(u >= 0.0)
}

impl Template {
    /// The normal crown: both sides fall away from the centerline.
    fn crowned(mut self) -> Template {
        self.lane = [-self.crossfall; 2];
        self.shoulder = [-self.shoulder_fall; 2];
        self
    }
    /// The road banked with the given lane slopes (outward rise per run, left and right). A shoulder
    /// that rises follows the lane; one that falls drops at least as steeply as the normal shoulder.
    fn banked(mut self, left: f64, right: f64) -> Template {
        self.lane = [left, right];
        let sh = |lane: f64| if lane >= 0.0 { lane } else { lane.min(-self.shoulder_fall) };
        self.shoulder = [sh(left), sh(right)];
        self
    }
    /// The design elevation at the signed offset `u` from the centerline.
    fn z(&self, centre: f64, u: f64) -> f64 {
        let k = side_of(u);
        let a = u.abs();
        let lane = self.lane.get(k).copied().unwrap_or(0.0);
        let shoulder = self.shoulder.get(k).copied().unwrap_or(0.0);
        centre + lane * a.min(self.lanes_half) + shoulder * (a.min(self.half) - self.lanes_half).max(0.0)
    }
    /// The run from the shoulder edge to each corner of the ditch (the foot of the inner slope,
    /// then the end of the bottom) on a cut side.
    fn ditch_breaks(&self, fill: bool) -> Vec<f64> {
        if fill || self.ditch_depth <= 0.0 {
            return Vec::new();
        }
        let d1 = self.ditch_depth * self.ditch_slope;
        vec![d1, d1 + self.ditch_width]
    }
    /// The design surface at `a` from the centreline on side `k`, the ditch and side slopes included.
    fn full(&self, centre: f64, k: usize, a: f64, fill: bool) -> f64 {
        let sign = if k == 0 { -1.0 } else { 1.0 };
        if a <= self.half {
            return self.z(centre, sign * a);
        }
        let ze = self.z(centre, sign * self.half);
        let run = a - self.half;
        if fill {
            return ze - run / self.fill_slope;
        }
        match self.ditch_breaks(false).as_slice() {
            [d1, d2] => {
                if run <= *d1 {
                    ze - run / self.ditch_slope
                } else if run <= *d2 {
                    ze - self.ditch_depth
                } else {
                    ze - self.ditch_depth + (run - d2) / self.cut_slope
                }
            }
            _ => ze + run / self.cut_slope,
        }
    }
}

/// The superelevation table `[[station, left %, right %], ...]`: sorted, finite, no repeated station.
fn parse_super(p: &Value) -> Result<Vec<(f64, f64, f64)>> {
    let Some(v) = p.get("superelevation").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let bad_rows = || bad("roadsection", "`superelevation` is [[station, left slope %, right slope %], ...] with slopes within 30 %");
    let a = v.as_array().ok_or_else(bad_rows)?;
    if a.len() > MAX_PVIS {
        return Err(bad("roadsection", format!("`superelevation` has at most {MAX_PVIS} rows")));
    }
    let mut out = Vec::with_capacity(a.len());
    for r in a {
        let f: Vec<f64> = r.as_array().ok_or_else(bad_rows)?.iter().filter_map(Value::as_f64).collect();
        let [sta, l, rt] = f[..] else { return Err(bad_rows()) };
        if !(sta.is_finite() && sta.abs() <= MAX_COORD && l.abs() <= 30.0 && rt.abs() <= 30.0) {
            return Err(bad_rows());
        }
        out.push((sta, l / 100.0, rt / 100.0));
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    if out.windows(2).any(|w| w.get(1).zip(w.first()).is_some_and(|(b, a)| b.0 - a.0 < 1e-9)) {
        return Err(bad("roadsection", "two `superelevation` rows at the same station"));
    }
    Ok(out)
}

/// The lane slopes at a station, interpolated linearly between rows; `None` outside the table
/// (the normal crown).
fn super_at(table: &[(f64, f64, f64)], sta: f64) -> Option<(f64, f64)> {
    let (first, last) = (table.first()?, table.last()?);
    if sta < first.0 - 1e-9 || sta > last.0 + 1e-9 {
        return None;
    }
    let i = table.partition_point(|r| r.0 <= sta).saturating_sub(1);
    let a = table.get(i)?;
    let b = table.get(i + 1).unwrap_or(a);
    let t = if b.0 - a.0 > 1e-12 { ((sta - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
    Some((a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t))
}

/// A region between the design and the ground: `fill` when the design is above, the points along
/// the design, then the points along the ground, both left to right as (offset, elevation).
struct Region {
    fill: bool,
    design: Vec<(f64, f64)>,
    ground: Vec<(f64, f64)>,
}

/// The regions between the design and the ground in samples `(offset, Some((design, ground)))`,
/// cut where the design is below. Crossings are split exactly; a missing ground ends a region.
fn regions(samples: &[(f64, Option<(f64, f64)>)]) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    let mut cur: Option<Region> = None;
    let flush = |cur: &mut Option<Region>, out: &mut Vec<Region>| {
        if let Some(r) = cur.take()
            && r.design.len() >= 2
        {
            out.push(r);
        }
    };
    for w in samples.windows(2) {
        let (Some((u0, Some((d0, g0)))), Some((u1, Some((d1, g1))))) = (w.first().copied(), w.get(1).copied()) else {
            flush(&mut cur, &mut out);
            continue;
        };
        let (a, b) = (d0 - g0, d1 - g1);
        // The pieces of this step: (from, to, fill), each end as (offset, design, ground).
        let mut pieces: Vec<((f64, f64, f64), (f64, f64, f64), bool)> = Vec::new();
        if (a > 0.0 && b < 0.0) || (a < 0.0 && b > 0.0) {
            let t = a / (a - b);
            let (um, zm) = (u0 + t * (u1 - u0), d0 + t * (d1 - d0));
            pieces.push(((u0, d0, g0), (um, zm, zm), a > 0.0));
            pieces.push(((um, zm, zm), (u1, d1, g1), b > 0.0));
        } else if a != 0.0 || b != 0.0 {
            pieces.push(((u0, d0, g0), (u1, d1, g1), a + b > 0.0));
        }
        for (p0, p1, fill) in pieces {
            let joins = cur.as_ref().is_some_and(|r| r.fill == fill && r.design.last().is_some_and(|l| (l.0 - p0.0).abs() < 1e-9));
            if !joins {
                flush(&mut cur, &mut out);
                cur = Some(Region { fill, design: vec![(p0.0, p0.1)], ground: vec![(p0.0, p0.2)] });
            }
            if let Some(r) = cur.as_mut() {
                r.design.push((p1.0, p1.1));
                r.ground.push((p1.0, p1.2));
            }
        }
        if a == 0.0 && b == 0.0 {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// The areas above and below zero of a sampled difference `d(u)` (design minus ground):
/// (fill, cut) where the design is above, or below, the ground. Crossings are split exactly.
fn areas(diffs: &[(f64, Option<f64>)]) -> (f64, f64) {
    let (mut fill, mut cut) = (0.0, 0.0);
    for w in diffs.windows(2) {
        let (Some((u0, Some(a))), Some((u1, Some(b)))) = (w.first().copied(), w.get(1).copied()) else { continue };
        let h = u1 - u0;
        if a >= 0.0 && b >= 0.0 {
            fill += (a + b) / 2.0 * h;
        } else if a <= 0.0 && b <= 0.0 {
            cut += -(a + b) / 2.0 * h;
        } else {
            let t = a / (a - b); // where it crosses zero
            let (h1, h2) = (h * t, h * (1.0 - t));
            if a > 0.0 {
                fill += a / 2.0 * h1;
                cut += -b / 2.0 * h2;
            } else {
                cut += -a / 2.0 * h1;
                fill += b / 2.0 * h2;
            }
        }
    }
    (fill, cut)
}

struct RoadSec {
    sta: f64,
    pos: Vec2,
    normal: Vec2,
    centre: f64,
    ground0: Option<f64>,
    /// Daylight offset (distance from the centerline) on each side: left, right.
    day: [Option<f64>; 2],
    /// `true` where the side is a fill.
    fill_side: [bool; 2],
    ground: Vec<Option<f64>>,
    fill: f64,
    cut: f64,
    ok: bool,
    /// The template at this station (banked where the superelevation says so).
    tpl: Template,
    /// The cut and fill regions, for shading.
    shading: Vec<Region>,
}

fn run_roadsection(s: &mut Session, p: &Value) -> Result<Value> {
    let name =
        str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("roadsection", "`name` (a centerline) is required"))?;
    let line = centerline(s, &json!({ "name": name }))?;
    let cum = cumulative(&line);
    let total = cum.last().copied().unwrap_or(0.0);
    if !(total.is_finite() && total > 0.0 && total <= MAX_COORD) {
        return Err(bad("roadsection", "the centerline has no usable length"));
    }
    let start = f64_or(p, "startStation", 0.0);
    let grade = grade_for(s, p, "roadsection")?
        .ok_or_else(|| bad("roadsection", "no design grade: run ROADPROFILE with `pvis` for this centerline first, or give `pvis`"))?;
    let interval = size_param("roadsection", p, "interval", false)?.unwrap_or(20.0);
    let lanes = match p.get("lanes") {
        None | Some(Value::Null) => 1.0,
        Some(v) => v.as_u64().filter(|n| (1..=12).contains(n)).ok_or_else(|| bad("roadsection", "`lanes` must be 1 to 12"))? as f64,
    };
    let lane_w = size_param("roadsection", p, "laneWidth", false)?.unwrap_or(3.5);
    let shoulder = size_param("roadsection", p, "shoulder", true)?.unwrap_or(1.5);
    let crossfall = size_param("roadsection", p, "crossfall", true)?.unwrap_or(2.0) / 100.0;
    let shoulder_fall = size_param("roadsection", p, "shoulderFall", true)?.unwrap_or(4.0) / 100.0;
    let cut_slope = size_param("roadsection", p, "cutSlope", false)?.unwrap_or(1.5);
    let fill_slope = size_param("roadsection", p, "fillSlope", false)?.unwrap_or(2.0);
    let reach = size_param("roadsection", p, "reach", false)?.unwrap_or(60.0);
    let ditch_depth = size_param("roadsection", p, "ditchDepth", true)?.unwrap_or(0.0);
    let ditch_width = size_param("roadsection", p, "ditchWidth", true)?.unwrap_or(0.6);
    let ditch_slope = size_param("roadsection", p, "ditchSlope", false)?.unwrap_or(3.0);
    let supers = parse_super(p)?;
    let shade = bool_or(p, "shade", true);
    let vscale = size_param("roadsection", p, "vScale", false)?.unwrap_or(1.0);
    if ![interval, lane_w, shoulder, crossfall, shoulder_fall, cut_slope, fill_slope, reach, vscale, start, ditch_depth, ditch_width, ditch_slope]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1.0e6)
    {
        return Err(bad("roadsection", "sizes must be finite and reasonable"));
    }
    let lanes_half = lanes * lane_w;
    let base_tpl = Template {
        lanes_half,
        half: lanes_half + shoulder,
        crossfall,
        shoulder_fall,
        cut_slope,
        fill_slope,
        ditch_depth,
        ditch_width,
        ditch_slope,
        lane: [0.0; 2],
        shoulder: [0.0; 2],
    }
    .crowned();
    let tpl = base_tpl;
    let half_sheet = tpl.half + reach + ditch_depth * ditch_slope + ditch_width;
    let span = 2.0 * half_sheet;
    let step = size_param("roadsection", p, "step", false)?.unwrap_or(span / 200.0);
    let th = size_param("roadsection", p, "textHeight", false)?.unwrap_or(span / 40.0);
    let columns = match p.get("columns") {
        None | Some(Value::Null) => 3usize,
        Some(v) => v
            .as_u64()
            .filter(|c| (1..=50).contains(c))
            .and_then(|c| usize::try_from(c).ok())
            .ok_or_else(|| bad("roadsection", "`columns` must be 1 to 50"))?,
    };
    let per = (half_sheet / step).ceil();
    if !(per.is_finite() && (1.0..=4000.0).contains(&per)) {
        return Err(bad("roadsection", "too many samples: use a larger `step`"));
    }
    let per = per as usize;
    let dstep = half_sheet / per as f64;
    let prefix = layer_name("roadsection", str_param(p, "layerPrefix").unwrap_or("RS"))?;

    // The stations within the design grade and the centerline.
    let (lo, hi) = (grade.first().max(start), grade.last().min(start + total));
    if lo > hi {
        return Err(bad("roadsection", "the design grade is outside the centerline's stations (check `startStation`)"));
    }
    let n = ((hi - lo) / interval).floor();
    if !(n.is_finite() && n < MAX_STATIONS as f64) {
        return Err(bad("roadsection", format!("more than {MAX_STATIONS} sections: use a larger `interval`")));
    }
    let mut stas: Vec<f64> = (0..=(n as usize)).map(|i| lo + i as f64 * interval).collect();
    if hi - stas.last().copied().unwrap_or(lo) > 1e-6 {
        stas.push(hi);
    }

    let tris = surface(s, &handles_param(p), "TIN")?;
    let index = Index::new(&tris).ok_or_else(|| bad("roadsection", "the surface is too detailed or empty"))?;

    let mut secs: Vec<RoadSec> = Vec::with_capacity(stas.len());
    for &sta in &stas {
        let (pos, dir) = locate(&line, &cum, sta - start);
        let normal = Vec2::new(-dir.y, dir.x);
        let centre = grade.z(sta).unwrap_or(0.0);
        let tpl = match super_at(&supers, sta) {
            Some((l, r)) => base_tpl.banked(l, r),
            None => base_tpl,
        };
        let world = |u: f64| pos - normal * u; // u < 0 is left of the direction of travel
        let gz = |u: f64| {
            let q = world(u);
            index.z(q.x, q.y)
        };
        let mut sec = RoadSec {
            sta,
            pos,
            normal,
            centre,
            ground0: gz(0.0),
            day: [None, None],
            fill_side: [false, false],
            ground: Vec::new(),
            fill: 0.0,
            cut: 0.0,
            ok: true,
            tpl,
            shading: Vec::new(),
        };
        // The daylight on each side: where the side slope meets the ground.
        for (k, sign) in [-1.0f64, 1.0].into_iter().enumerate() {
            let ze = tpl.z(centre, sign * tpl.half);
            let Some(g_edge) = gz(sign * tpl.half) else {
                sec.ok = false;
                continue;
            };
            let fill = g_edge < ze;
            if let Some(f) = sec.fill_side.get_mut(k) {
                *f = fill;
            }
            let diff = |d: f64| -> Option<f64> { Some(tpl.full(centre, k, tpl.half + d, fill) - gz(sign * (tpl.half + d))?) };
            let mut prev = (0.0, ze - g_edge);
            let mut found = None;
            if prev.1 == 0.0 {
                found = Some(0.0);
            } else {
                for i in 1..=per {
                    let d = i as f64 * reach / per as f64;
                    let Some(v) = diff(d) else { break };
                    if (prev.1 < 0.0) != (v < 0.0) || v == 0.0 {
                        let t = prev.1 / (prev.1 - v);
                        found = Some(prev.0 + t * (d - prev.0));
                        break;
                    }
                    prev = (d, v);
                }
            }
            if let Some(d) = found
                && let Some(slot) = sec.day.get_mut(k)
            {
                *slot = Some(tpl.half + d);
            }
        }
        // Areas between the design (with its slopes) and the ground.
        if let (Some(l), Some(r)) = (sec.day[0], sec.day[1]) {
            let m = ((l + r) / dstep.max(1e-9)).ceil().clamp(2.0, 20_000.0) as usize;
            let pairs: Vec<(f64, Option<(f64, f64)>)> = (0..=m)
                .map(|i| {
                    let u = -l + (l + r) * i as f64 / m as f64;
                    let side = side_of(u);
                    let fill = sec.fill_side.get(side).copied().unwrap_or(false);
                    (u, gz(u).map(|g| (tpl.full(centre, side, u.abs(), fill), g)))
                })
                .collect();
            let samples: Vec<(f64, Option<f64>)> = pairs.iter().map(|(u, v)| (*u, v.map(|(d, g)| d - g))).collect();
            let (f, c) = areas(&samples);
            sec.fill = f;
            sec.cut = c;
            if shade {
                sec.shading = regions(&pairs);
            }
        } else {
            sec.ok = false;
        }
        sec.ground = (0..=(2 * per)).map(|i| gz(-half_sheet + span * i as f64 / (2 * per) as f64)).collect();
        secs.push(sec);
    }

    // Volumes by average end area.
    let mut table = Vec::new();
    let (mut cum_cut, mut cum_fill) = (0.0, 0.0);
    let mut seg_vols: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    for w in secs.windows(2) {
        if let [a, b] = w {
            let d = b.sta - a.sta;
            let (vc, vf) = if a.ok && b.ok { ((a.cut + b.cut) / 2.0 * d, (a.fill + b.fill) / 2.0 * d) } else { (0.0, 0.0) };
            cum_cut += vc;
            cum_fill += vf;
            seg_vols.push((vc, vf));
        }
    }
    let mut run_cut = 0.0;
    let mut run_fill = 0.0;
    for (sec, (vc, vf)) in secs.iter().zip(&seg_vols) {
        run_cut += vc;
        run_fill += vf;
        table.push(json!({
            "station": sec.sta, "ground": sec.ground0, "design": sec.centre, "cut": sec.cut, "fill": sec.fill,
            "daylightLeft": sec.day[0], "daylightRight": sec.day[1], "complete": sec.ok,
            "slopeLeft": sec.tpl.lane[0] * 100.0, "slopeRight": sec.tpl.lane[1] * 100.0,
            "cutVolume": vc, "fillVolume": vf, "cumCut": run_cut, "cumFill": run_fill,
        }));
    }
    let incomplete = secs.iter().filter(|x| !x.ok).count();

    // Drawing: a sheet of sections, the road edges on the plan and a quantity table.
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for sec in &secs {
        let t = &sec.tpl;
        for z in sec.ground.iter().flatten().copied().chain([sec.centre, t.z(sec.centre, -t.half), t.z(sec.centre, t.half)]) {
            zmin = zmin.min(z);
            zmax = zmax.max(z);
        }
        for (k, d) in sec.day.iter().enumerate() {
            let fill = sec.fill_side.get(k).copied().unwrap_or(false);
            if let Some(d) = d {
                let z = t.full(sec.centre, k, *d, fill);
                zmin = zmin.min(z);
                zmax = zmax.max(z);
            }
            if !fill && t.ditch_depth > 0.0 {
                zmin = zmin.min(t.z(sec.centre, if k == 0 { -t.half } else { t.half }) - t.ditch_depth);
            }
        }
    }
    if !(zmin.is_finite() && zmax.is_finite()) {
        return Err(bad("roadsection", "the centerline does not cross the surface"));
    }
    let range = (zmax - zmin).max(1e-9);
    let vstep = nice_spacing(range);
    let datum = ((zmin - 0.1 * range) / vstep).floor() * vstep;
    let pad = th * 5.0;
    let (cw, ch) = (span + 2.0 * pad, (zmax - datum) * vscale + 2.0 * pad);
    let gap = th * 3.0;
    let origin = sheet_origin(s, p, "roadsection", cw)?;
    if !(cw.is_finite() && ch.is_finite()) {
        return Err(bad("roadsection", "the sheet would be too large"));
    }
    let (l_ground, l_design, l_frame, l_text, l_cl) =
        (format!("{prefix}-GROUND"), format!("{prefix}-DESIGN"), format!("{prefix}-FRAME"), format!("{prefix}-TEXT"), format!("{prefix}-CL"));
    let (l_edge, l_day) = (format!("{prefix}-PLAN-EDGE"), format!("{prefix}-PLAN-DAYLIGHT"));
    let (l_cut, l_fill) = (format!("{prefix}-CUT"), format!("{prefix}-FILL"));
    let mut items: Vec<(&str, EntityKind)> = Vec::new();
    for (k, sec) in secs.iter().enumerate() {
        let (col, row) = (k % columns, k / columns);
        let top = Vec2::new(origin.x + col as f64 * (cw + gap), origin.y - row as f64 * (ch + gap));
        let (x0, base) = (top.x + pad, top.y - ch + pad);
        let xc = x0 + half_sheet;
        let at = |u: f64, z: f64| Vec2::new(xc + u, base + (z - datum) * vscale);
        items.push((
            &l_frame,
            polyline(vec![top, Vec2::new(top.x + cw, top.y), Vec2::new(top.x + cw, top.y - ch), Vec2::new(top.x, top.y - ch)], true),
        ));
        items.push((&l_frame, seg(Vec2::new(x0, base), Vec2::new(x0 + span, base))));
        items.push((&l_cl, seg(Vec2::new(xc, base), Vec2::new(xc, top.y - pad * 0.5))));
        let mut run: Vec<Vec2> = Vec::new();
        for (j, z) in sec.ground.iter().enumerate() {
            match z {
                Some(z) => run.push(Vec2::new(x0 + span * j as f64 / (2 * per) as f64, base + (z - datum) * vscale)),
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
        // The design: left daylight, shoulder edge, crown, shoulder edge, right daylight.
        let t = &sec.tpl;
        let (dl, dr) = (sec.day[0].unwrap_or(t.half), sec.day[1].unwrap_or(t.half));
        let (fl, fr) = (sec.fill_side[0], sec.fill_side[1]);
        // The corners of a side outside the shoulder edge: the ditch, then the daylight.
        let outer = |day: f64, fill: bool| -> Vec<f64> {
            let mut v: Vec<f64> = t.ditch_breaks(fill).into_iter().map(|r| t.half + r).filter(|a| *a < day - 1e-9).collect();
            v.push(day);
            v
        };
        let mut dsg: Vec<Vec2> = Vec::new();
        for a in outer(dl, fl).into_iter().rev() {
            dsg.push(at(-a, t.full(sec.centre, 0, a, fl)));
        }
        for u in [-t.half, -t.lanes_half, 0.0, t.lanes_half, t.half] {
            dsg.push(at(u, t.z(sec.centre, u)));
        }
        for a in outer(dr, fr) {
            dsg.push(at(a, t.full(sec.centre, 1, a, fr)));
        }
        items.push((&l_design, polyline(dsg, false)));
        for reg in &sec.shading {
            let verts: Vec<PolyVertex> = reg.design.iter().chain(reg.ground.iter().rev()).map(|&(u, z)| PolyVertex::new(at(u, z))).collect();
            let (layer, angle) = if reg.fill { (&l_fill, 90.0_f64.to_radians()) } else { (&l_cut, 0.0) };
            items.push((layer, hatch_region(verts, "ANSI31", angle, th * 0.6 / 0.125)));
        }
        let label = format!("STA {}", station(sec.sta));
        items.push((&l_text, txt(s, Vec2::new(top.x + pad, top.y - pad * 0.6), &label, th * 1.4)));
        items.push((&l_text, txt(s, Vec2::new(top.x + th * 0.5, base - th * 2.8), &format!("Cut {:.2} m2   Fill {:.2} m2", sec.cut, sec.fill), th)));
        items.push((
            &l_text,
            txt(
                s,
                Vec2::new(top.x + th * 0.5, base - th * 4.4),
                &format!("Design {:.2}   Ground {}", sec.centre, sec.ground0.map_or("-".to_string(), |g| format!("{g:.2}"))),
                th,
            ),
        ));
        if !supers.is_empty() {
            let txt_s = format!("Slope L {:.1}%  R {:.1}%", t.lane[0] * 100.0, t.lane[1] * 100.0);
            items.push((&l_text, txt(s, Vec2::new(top.x + th * 0.5, base - th * 7.6), &txt_s, th)));
        }
        if !sec.ok {
            items.push((&l_text, txt(s, Vec2::new(top.x + th * 0.5, base - th * 6.0), "No daylight: the side slope does not meet the ground", th)));
        }
        if let Some(d) = sec.day[0] {
            items.push((&l_text, txt(s, at(-d, t.full(sec.centre, 0, d, fl)) + Vec2::new(0.0, th), &format!("{d:.1}"), th)));
        }
        if let Some(d) = sec.day[1] {
            items.push((&l_text, txt(s, at(d, t.full(sec.centre, 1, d, fr)) + Vec2::new(0.0, th), &format!("{d:.1}"), th)));
        }
    }
    // The plan: the road edges and the daylight lines.
    let mut edges: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
    let mut days: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
    for sec in &secs {
        for (k, sign) in [-1.0f64, 1.0].into_iter().enumerate() {
            if let Some(e) = edges.get_mut(k) {
                e.push(sec.pos - sec.normal * (sign * tpl.half));
            }
            if let (Some(d), Some(v)) = (sec.day.get(k).copied().flatten(), days.get_mut(k)) {
                v.push(sec.pos - sec.normal * (sign * d));
            }
        }
    }
    for e in edges.into_iter() {
        if e.len() >= 2 {
            items.push((&l_edge, polyline(e, false)));
        }
    }
    for e in days.into_iter() {
        if e.len() >= 2 {
            items.push((&l_day, polyline(e, false)));
        }
    }
    // The quantity table under the sheet.
    let rows = secs.len().div_ceil(columns);
    let mut y = origin.y - rows as f64 * (ch + gap) - th * 2.0;
    let cols = ["STATION", "CUT m2", "FILL m2", "CUT m3", "FILL m3", "CUM CUT", "CUM FILL"];
    let w = th * 14.0;
    for (j, c) in cols.iter().enumerate() {
        items.push((&l_text, txt(s, Vec2::new(origin.x + j as f64 * w, y), c, th * 1.2)));
    }
    y -= th * 2.0;
    for (row, (sec, (vc, vf))) in secs.iter().zip(&seg_vols).enumerate() {
        let cumc: f64 = seg_vols.iter().take(row + 1).map(|v| v.0).sum();
        let cumf: f64 = seg_vols.iter().take(row + 1).map(|v| v.1).sum();
        let cells = [
            station(sec.sta),
            format!("{:.2}", sec.cut),
            format!("{:.2}", sec.fill),
            format!("{vc:.1}"),
            format!("{vf:.1}"),
            format!("{cumc:.1}"),
            format!("{cumf:.1}"),
        ];
        for (j, c) in cells.iter().enumerate() {
            items.push((&l_text, txt(s, Vec2::new(origin.x + j as f64 * w, y), c, th)));
        }
        y -= th * 1.6;
    }
    items.push((&l_text, txt(s, Vec2::new(origin.x, y - th * 0.5), &format!("TOTAL  CUT {cum_cut:.1} m3   FILL {cum_fill:.1} m3"), th * 1.3)));

    let space = s.space();
    let base_common = s.current_common();
    let n_items = items.len();
    {
        let d = s.doc_mut()?;
        layer_colors(
            d,
            &[(&l_ground, 3), (&l_design, 1), (&l_frame, 8), (&l_text, 7), (&l_cl, 2), (&l_edge, 1), (&l_day, 5), (&l_cut, 30), (&l_fill, 150)],
        );
        for (layer, kind) in items {
            let com = Common { layer: layer.to_string(), color: Color::ByLayer, ..base_common.clone() };
            d.add(&space, com, kind)?;
        }
    }
    let msg = format!(
        "{} road sections every {interval}: cut {cum_cut:.1}, fill {cum_fill:.1} (average end area){}",
        secs.len(),
        if incomplete > 0 { format!("; {incomplete} sections do not meet the ground within {reach}") } else { String::new() }
    );
    s.echo(msg.clone());
    let _ = CENTERLINE_PREFIX;
    Ok(json!({
        "sections": table, "cutVolume": cum_cut, "fillVolume": cum_fill, "incomplete": incomplete,
        "sheetAt": [origin.x, origin.y], "cell": [cw, ch], "objects": n_items, "message": msg,
    }))
}
