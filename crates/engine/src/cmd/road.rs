//! ROADALIGN: a horizontal road alignment. Click the points of intersection (PIs); every PI with a
//! radius is rounded by a circular curve tangent to both legs (tangent length T = R tan(Δ/2)), and
//! the result is one polyline (lines and arcs) on the layer of a named centerline, so Cross
//! Sections can cut along it. Stations count along the alignment from `startStation`; the command
//! lists each curve (PI, TC and CT stations, R, Δ, T, L, E) and can mark the stations.

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, HAlign, Handle, Line, LwPolyline, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::mapgrid::nice_spacing;
use super::xsection::{centerline_layer, next_name};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most PIs, most station ticks and largest coordinate or radius accepted.
const MAX_PIS: usize = 10_000;
const MAX_TICKS: f64 = 5000.0;
const MAX_COORD: f64 = 1.0e12;
/// The radius given to a PI that has none.
const DEFAULT_RADIUS: f64 = 100.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("roadalign", "Horizontal Alignment...", run_align)
            .alias(&["alignmentcurves", "horizontalcurves", "hcurve"])
            .menu(&["Tools", "Road", "Horizontal Alignment..."])
            .params(
                "{pis?: [[x, y] or [x, y, radius], ...] (the first and last are the ends, the others the PIs), from?: handle of a line or polyline \
                 on any layer whose vertices are the PIs (instead of `pis`), fromName?: the name of a centerline to take the PIs from, radius?: for PIs without one \
                 (default 100; 0 leaves a sharp corner), name?: the centerline's name (default: the next Line 1A, Line 2A, ...), \
                 startStation?: 0, ticks?: draw station marks every this much (true: a round number), textHeight?, labels?: true (curve data at the PIs)} \
                 → {name, length, curves: [{pi, tc, ct, radius, delta, tangent, length, external, piStation, tcStation, ctStation}]}",
            )
            .interactive(|_| Ok(Box::new(AlignM::default()))),
    ]
}

/// One piece of the alignment, for stations and directions.
#[derive(Clone, Copy)]
enum El {
    Line {
        a: Vec2,
        b: Vec2,
        len: f64,
    },
    /// An arc from angle `a0` turning `sweep` radians (positive counter-clockwise).
    Arc {
        c: Vec2,
        r: f64,
        a0: f64,
        sweep: f64,
        len: f64,
    },
}

impl El {
    fn len(&self) -> f64 {
        match self {
            El::Line { len, .. } | El::Arc { len, .. } => *len,
        }
    }
    /// Position and unit direction `d` along the piece.
    fn at(&self, d: f64) -> (Vec2, Vec2) {
        match *self {
            El::Line { a, b, len } => {
                let dir = (b - a).normalized();
                (a + dir * d.clamp(0.0, len), dir)
            }
            El::Arc { c, r, a0, sweep, len } => {
                let t = if len > 0.0 { (d / len).clamp(0.0, 1.0) } else { 0.0 };
                let ang = a0 + sweep * t;
                let radial = Vec2::from_angle(ang);
                (c + radial * r, if sweep >= 0.0 { radial.perp() } else { radial.perp() * -1.0 })
            }
        }
    }
}

/// A circular curve of the alignment.
struct Curve {
    pi: Vec2,
    tc: Vec2,
    ct: Vec2,
    radius: f64,
    delta: f64,
    tangent: f64,
    length: f64,
    external: f64,
    pi_sta: f64,
    tc_sta: f64,
    ct_sta: f64,
}

/// Round the PIs. `pts[0]` and `pts[last]` are the ends; `radii[i]` is the radius at `pts[i]` (0: a corner).
fn build(pts: &[Vec2], radii: &[f64], start: f64) -> Result<(Vec<PolyVertex>, Vec<El>, Vec<Curve>)> {
    let n = pts.len();
    // Directions, deflections and tangent lengths.
    let mut tangent = vec![0.0; n];
    let mut delta = vec![0.0; n];
    for i in 1..n.saturating_sub(1) {
        let (d1, d2) = ((pts[i] - pts[i - 1]).normalized(), (pts[i + 1] - pts[i]).normalized());
        let dl = d1.cross(d2).abs().atan2(d1.dot(d2));
        delta[i] = dl;
        if radii[i] > 0.0 && dl > 1e-9 {
            if dl > std::f64::consts::PI - 1e-6 {
                return Err(bad("roadalign", format!("PI {i}: the road turns back on itself")));
            }
            tangent[i] = radii[i] * (dl / 2.0).tan();
        }
    }
    for i in 0..n - 1 {
        let room = pts[i].dist(pts[i + 1]);
        if tangent[i] + tangent[i + 1] > room * (1.0 - 1e-9) {
            let at = if tangent[i] >= tangent[i + 1] { i } else { i + 1 };
            return Err(bad(
                "roadalign",
                format!("PI {at}: the radius {:.2} is too large for the legs on either side (tangent {:.2})", radii[at], tangent[at]),
            ));
        }
    }
    let mut verts = vec![PolyVertex::new(pts[0])];
    let (mut els, mut curves) = (Vec::new(), Vec::new());
    let (mut prev, mut s) = (pts[0], start);
    for i in 1..n - 1 {
        if tangent[i] <= 0.0 {
            els.push(El::Line { a: prev, b: pts[i], len: prev.dist(pts[i]) });
            s += prev.dist(pts[i]);
            verts.push(PolyVertex::new(pts[i]));
            prev = pts[i];
            continue;
        }
        let (d1, d2) = ((pts[i] - pts[i - 1]).normalized(), (pts[i + 1] - pts[i]).normalized());
        let left = d1.cross(d2) > 0.0;
        let (tc, ct) = (pts[i] - d1 * tangent[i], pts[i] + d2 * tangent[i]);
        let r = radii[i];
        let center = tc + if left { d1.perp() } else { d1.perp() * -1.0 } * r;
        let sign = if left { 1.0 } else { -1.0 };
        let arc_len = r * delta[i];
        els.push(El::Line { a: prev, b: tc, len: prev.dist(tc) });
        s += prev.dist(tc);
        let tc_sta = s;
        els.push(El::Arc { c: center, r, a0: center.angle_to(tc), sweep: sign * delta[i], len: arc_len });
        s += arc_len;
        verts.push(PolyVertex::with_bulge(tc, sign * (delta[i] / 4.0).tan()));
        verts.push(PolyVertex::new(ct));
        curves.push(Curve {
            pi: pts[i],
            tc,
            ct,
            radius: r,
            delta: delta[i],
            tangent: tangent[i],
            length: arc_len,
            external: r * (1.0 / (delta[i] / 2.0).cos() - 1.0),
            pi_sta: tc_sta + tangent[i],
            tc_sta,
            ct_sta: s,
        });
        prev = ct;
    }
    els.push(El::Line { a: prev, b: pts[n - 1], len: prev.dist(pts[n - 1]) });
    verts.push(PolyVertex::new(pts[n - 1]));
    Ok((verts, els, curves))
}

/// Position and direction at distance `d` from the start of the alignment.
fn locate(els: &[El], d: f64) -> Option<(Vec2, Vec2)> {
    let mut left = d;
    for (k, e) in els.iter().enumerate() {
        if left <= e.len() || k + 1 == els.len() {
            return Some(e.at(left));
        }
        left -= e.len();
    }
    None
}

/// `1+250.00`: kilometres + metres.
fn station(s: f64) -> String {
    format!("{}+{:06.2}", (s / 1000.0).floor() as i64, s.rem_euclid(1000.0))
}

fn point3(v: &Value) -> Option<(Vec2, Option<f64>)> {
    let a = v.as_array()?;
    let (x, y) = (a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    let r = match a.get(2) {
        None | Some(Value::Null) => None,
        Some(r) => Some(r.as_f64().filter(|r| r.is_finite() && *r >= 0.0 && *r <= MAX_COORD)?),
    };
    (x.is_finite() && y.is_finite() && x.abs() <= MAX_COORD && y.abs() <= MAX_COORD).then_some((Vec2::new(x, y), r))
}

/// The vertices of a line or polyline on any layer, where its PIs are (an arc in it counts as a straight leg).
fn vertices_of(s: &Session, h: Handle) -> Option<Vec<Vec2>> {
    let d = s.doc().ok()?;
    let pts: Vec<Vec2> = match &d.entity(h)?.kind {
        EntityKind::Line(l) => vec![Vec2::new(l.a.x, l.a.y), Vec2::new(l.b.x, l.b.y)],
        EntityKind::LwPolyline(pl) => pl.vertices.iter().map(|v| v.p).collect(),
        EntityKind::Polyline3d(pl) => pl.points.iter().map(|p| Vec2::new(p.x, p.y)).collect(),
        _ => return None,
    };
    (pts.len() <= MAX_PIS).then_some(pts)
}

fn run_align(s: &mut Session, p: &Value) -> Result<Value> {
    let source: Option<Vec<Vec2>> = if let Some(t) = str_param(p, "from") {
        let h = Handle::parse_hex(t).ok_or_else(|| bad("roadalign", "`from` is not a handle"))?;
        Some(vertices_of(s, h).ok_or_else(|| bad("roadalign", "`from` must be a line or polyline"))?)
    } else if let Some(n) = str_param(p, "fromName") {
        let layer = centerline_layer("roadalign", n)?;
        let found = s.doc()?.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(&layer)).find_map(|e| vertices_of(s, e.handle));
        Some(found.ok_or_else(|| bad("roadalign", format!("no centerline named \"{}\"", n.trim())))?)
    } else {
        None
    };
    let given: Vec<(Vec2, Option<f64>)> = match source {
        Some(v) => v.into_iter().map(|q| (q, None)).collect(),
        None => {
            let arr = p
                .get("pis")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("roadalign", "`pis` ([[x, y, radius?], ...]), `from` (a handle) or `fromName` is required"))?;
            if arr.len() > MAX_PIS {
                return Err(bad("roadalign", format!("`pis` needs 2 to {MAX_PIS} points")));
            }
            arr.iter()
                .map(|v| point3(v).ok_or_else(|| bad("roadalign", "`pis` must be [x, y] or [x, y, radius] numbers (radius zero or more)")))
                .collect::<Result<_>>()?
        }
    };
    if given.iter().any(|(q, _)| !(q.x.is_finite() && q.y.is_finite() && q.x.abs() <= MAX_COORD && q.y.abs() <= MAX_COORD)) {
        return Err(bad("roadalign", "the line has invalid coordinates"));
    }
    let default_r = match p.get("radius") {
        None | Some(Value::Null) => DEFAULT_RADIUS,
        Some(v) => {
            v.as_f64().filter(|r| r.is_finite() && *r >= 0.0 && *r <= MAX_COORD).ok_or_else(|| bad("roadalign", "`radius` must be zero or more"))?
        }
    };
    let mut pts: Vec<Vec2> = Vec::new();
    let mut radii: Vec<f64> = Vec::new();
    for (q, r) in given {
        if pts.last().is_some_and(|l| l.near(q, 1e-9)) {
            continue; // the same point twice
        }
        pts.push(q);
        radii.push(r.unwrap_or(default_r));
    }
    if pts.len() < 2 {
        return Err(bad("roadalign", "the alignment needs at least two different points"));
    }
    let start = f64_or(p, "startStation", 0.0);
    if start.abs() > MAX_COORD {
        return Err(bad("roadalign", "`startStation` is too large"));
    }
    let (verts, els, curves) = build(&pts, &radii, start)?;
    let total: f64 = els.iter().map(El::len).sum();
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => next_name(s),
    };
    let layer = centerline_layer("roadalign", &name)?;
    if s.doc()?.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer)) {
        return Err(bad("roadalign", format!("a centerline named \"{name}\" exists: use another name")));
    }
    let th = size_param("roadalign", p, "textHeight", false)?.unwrap_or_else(|| (total / 150.0).max(f64::MIN_POSITIVE));
    let ticks: Option<f64> = match p.get("ticks") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(nice_spacing(total)),
        Some(_) => size_param("roadalign", p, "ticks", false)?,
    };
    if let Some(t) = ticks
        && !(0.0..MAX_TICKS).contains(&(total / t))
    {
        return Err(bad("roadalign", "too many station marks: use a larger `ticks`"));
    }
    if !(total.is_finite() && th.is_finite()) {
        return Err(bad("roadalign", "the alignment is too large"));
    }

    // Draw.
    let labels = bool_or(p, "labels", true);
    let mut items: Vec<(String, EntityKind)> = Vec::new();
    items.push((
        layer.clone(),
        EntityKind::LwPolyline(LwPolyline { vertices: verts, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }),
    ));
    let v3 = |q: Vec2| Vec3::new(q.x, q.y, 0.0);
    if let Some(t) = ticks {
        // Marks at the multiples of `t` from the start station (and the ends), across the road.
        let first = (start / t).ceil() * t;
        let mut k = 0.0;
        loop {
            let sta = first + k * t;
            if sta > start + total + 1e-9 {
                break;
            }
            if let Some((pos, dir)) = locate(&els, (sta - start).max(0.0)) {
                let n = dir.perp();
                items.push(("ALIGN-STA".into(), EntityKind::Line(Line { a: v3(pos + n * th), b: v3(pos - n * th) })));
                items.push((
                    "ALIGN-STA".into(),
                    make_text(s, pos + n * (th * 1.3), &station(sta), th, dir.angle() - std::f64::consts::FRAC_PI_2, (HAlign::Left, VAlign::Middle)),
                ));
            }
            k += 1.0;
        }
    }
    let mut table = Vec::new();
    for (k, c) in curves.iter().enumerate() {
        let line = format!(
            "Curve {}: PI {} R={:.2} Δ={:.4}° T={:.2} L={:.2} E={:.2} TC {} CT {}",
            k + 1,
            station(c.pi_sta),
            c.radius,
            c.delta.to_degrees(),
            c.tangent,
            c.length,
            c.external,
            station(c.tc_sta),
            station(c.ct_sta)
        );
        if labels {
            let txt = format!("R={:.1} Δ={:.2}° L={:.1}", c.radius, c.delta.to_degrees(), c.length);
            items.push(("ALIGN-CURVES".into(), make_text(s, c.pi + Vec2::new(th, th), &txt, th, 0.0, (HAlign::Left, VAlign::Baseline))));
        }
        table.push(line);
    }
    let space = s.space();
    let base = s.current_common();
    {
        let d = s.doc_mut()?;
        for (l, c) in [(layer.as_str(), 2u8), ("ALIGN-STA", 7), ("ALIGN-CURVES", 8)] {
            if d.ensure_layer(l)
                && let Some(x) = d.layers.iter_mut().find(|x| x.name == l)
            {
                x.color = Color::Index(c);
            }
        }
        for (l, kind) in items {
            d.add(&space, Common { layer: l, color: Color::ByLayer, ..base.clone() }, kind)?;
        }
    }
    let msg = format!(
        "alignment {name}: {} PIs, {} curves, length {total:.2}, stations {} to {}",
        pts.len(),
        curves.len(),
        station(start),
        station(start + total)
    );
    s.echo(msg.clone());
    for l in &table {
        s.echo(l.clone());
    }
    let curve_json: Vec<Value> = curves
        .iter()
        .map(|c| {
            json!({
                "pi": [c.pi.x, c.pi.y], "tc": [c.tc.x, c.tc.y], "ct": [c.ct.x, c.ct.y], "radius": c.radius, "delta": c.delta.to_degrees(),
                "tangent": c.tangent, "length": c.length, "external": c.external,
                "piStation": c.pi_sta, "tcStation": c.tc_sta, "ctStation": c.ct_sta,
            })
        })
        .collect();
    Ok(
        json!({ "name": name, "layer": layer, "length": total, "startStation": start, "endStation": start + total, "curves": curve_json, "table": table, "message": msg }),
    )
}

/// ROADALIGN at the command line: click the PIs, then the radius and the name.
#[derive(Default)]
struct AlignM {
    pts: Vec<Vec2>,
    /// 0 points, 1 radius, 2 name.
    step: u8,
    radius: Option<f64>,
    /// The UI asked for its settings dialog after the PIs.
    dialog: bool,
    /// Picking an existing line or polyline for the PIs (a SELECT prompt makes a click on empty
    /// space start a window, so the plain start prompt takes points only).
    selecting: bool,
}

impl AlignM {
    /// With the UI's dialog wanted, hand the PIs over and end the command.
    fn hand_over(&self, s: &mut Session) -> bool {
        if self.dialog {
            s.align_pending = Some(self.pts.clone());
        }
        self.dialog
    }
}

impl Interactive for AlignM {
    fn name(&self) -> &'static str {
        "ROADALIGN"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.dialog = std::mem::take(&mut s.align_dialog);
        // A line or polyline already selected gives the PIs straight away.
        if let Some(v) = s.selection().into_iter().find_map(|h| vertices_of(s, h)).map(|v| distinct_points(&v)).filter(|v| v.len() >= 2) {
            self.pts = v;
            self.step = 1;
            s.set_selection(Vec::new());
            if self.hand_over(s) {
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.step {
            0 if self.pts.is_empty() && self.selecting => Prompt::new("Select a line or polyline for the PIs (any layer)", Accept::SELECT),
            0 if self.pts.is_empty() => Prompt::new("Specify the start of the alignment", Accept::POINT).kw(&["Select"]),
            0 => {
                let mut p = Prompt::new("Specify the next PI, or the end (Enter to finish)", Accept::POINT);
                p.base = self.pts.last().copied();
                p
            }
            1 => {
                let mut p = Prompt::new("Curve radius at the PIs (0: sharp corners)", Accept::TEXT);
                p.default = Some(DEFAULT_RADIUS.to_string());
                p
            }
            _ => {
                let mut p = Prompt::new("Name of the alignment", Accept::TEXT);
                p.default = Some(next_name(s));
                p
            }
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        if self.pts.is_empty() || self.step != 0 {
            return Vec::new();
        }
        let vs = self.pts.iter().copied().chain(std::iter::once(cursor)).map(cadcraft_geom::PolyVertex::new).collect();
        vec![super::helpers::lwpoly(vs, false)]
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.step, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (0, Input::Keyword(k)) if self.pts.is_empty() && k == "Select" => {
                self.selecting = true;
                Ok(Step::Continue)
            }
            (0, Input::Pick(hs)) if self.pts.is_empty() => {
                match hs.into_iter().find_map(|h| vertices_of(s, h)).map(|v| distinct_points(&v)).filter(|v| v.len() >= 2) {
                    Some(v) => {
                        self.pts = v;
                        self.step = 1;
                        if self.hand_over(s) {
                            return Ok(Step::Done);
                        }
                    }
                    None => s.echo("Pick a line or a polyline with at least two different points.".to_string()),
                }
                Ok(Step::Continue)
            }
            (0, Input::Point(q)) => {
                self.pts.push(q);
                Ok(Step::Continue)
            }
            (0, Input::Enter) => {
                if self.pts.len() < 2 {
                    s.echo("Click at least two points.".to_string());
                    return Ok(Step::Continue);
                }
                self.step = 1;
                if self.hand_over(s) {
                    return Ok(Step::Done);
                }
                Ok(Step::Continue)
            }
            (1, Input::Text(t)) if !t.trim().is_empty() => match t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0) {
                Some(v) => {
                    self.radius = Some(v);
                    self.step = 2;
                    Ok(Step::Continue)
                }
                None => {
                    s.echo("Enter zero or a positive number.".to_string());
                    Ok(Step::Continue)
                }
            },
            (1, Input::Enter) => {
                self.step = 2;
                Ok(Step::Continue)
            }
            (2, i) => {
                let name = if let Input::Text(t) = i { t.trim().to_string() } else { String::new() };
                let pis: Vec<[f64; 2]> = self.pts.iter().map(|q| [q.x, q.y]).collect();
                let mut params = json!({ "pis": pis, "name": name, "ticks": true });
                if let Some(r) = self.radius {
                    params["radius"] = json!(r);
                }
                match run_align(s, &params) {
                    Ok(_) => Ok(Step::Done),
                    Err(e) => {
                        s.echo(e.to_string());
                        Ok(Step::Continue)
                    }
                }
            }
            _ => Ok(Step::Continue),
        }
    }
}
