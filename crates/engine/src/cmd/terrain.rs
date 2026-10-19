//! Ground surfaces from survey points. TIN triangulates the points (Delaunay) into 3DFACE
//! objects, and CONTOUR draws elevation contours through that surface: polylines at their
//! elevation, major (index) contours thicker and labelled.

use std::collections::HashMap;

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, Face3d, HAlign, Handle, Lineweight, LwPolyline, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};
use spade::{DelaunayTriangulation, HasPosition, Point2, Triangulation};

use super::draw::make_text;
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most points triangulated and most triangles contoured by one command.
const MAX_POINTS: usize = 500_000;
const MAX_TRIANGLES: usize = 1_500_000;
/// Most contour segments one command produces (an interval far too small for the relief).
const MAX_SEGMENTS: usize = 4_000_000;
/// Largest coordinate magnitude accepted.
const MAX_COORD: f64 = 1.0e12;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tin", "Create TIN Surface...", run_tin)
            .alias(&["surface", "createtin"])
            .menu(&["Tools", "Terrain", "Create TIN Surface..."])
            .params(
                "{handles?: [POINT objects] (default: every point in the drawing), maxEdge?: drop triangles with a longer side \
                 (the empty spans over gaps and outside the survey), layer?: \"TIN\"} → 3DFACE triangles",
            )
            .interactive(|_| Ok(Box::new(TinM::default()))),
        CommandSpec::new("contour", "Contours...", run_contour)
            .alias(&["contours", "createcontours"])
            .menu(&["Tools", "Terrain", "Contours..."])
            .params(
                "{interval?: contour interval (default: a round number giving about 10 levels), base?: 0, majorEvery?: 5, \
                 smooth?: 0 to 4 rounds of corner rounding (default 2, 0 = straight lines), labels?: true, textHeight?, decimals?, handles?: [3DFACE triangles] (default: the TIN layer, else a TIN is \
                 built from the points), layerPrefix?: \"CONTOUR\"}",
            )
            .interactive(|_| Ok(Box::new(ContourM::default()))),
    ]
}

pub(super) fn finite3(p: &Vec3) -> bool {
    [p.x, p.y, p.z].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD)
}

struct Vtx {
    x: f64,
    y: f64,
    z: f64,
}

impl HasPosition for Vtx {
    type Scalar = f64;
    fn position(&self) -> Point2<f64> {
        Point2::new(self.x, self.y)
    }
}

/// POINT objects: the given ones, else every point in the drawing.
fn gather_points(s: &Session, handles: &[Handle]) -> Result<Vec<Vec3>> {
    let d = s.doc()?;
    let mut out = Vec::new();
    let mut take = |e: &cadcraft_doc::Entity| {
        if let EntityKind::Point(p) = &e.kind
            && finite3(&p.p)
        {
            out.push(p.p);
        }
    };
    if handles.is_empty() {
        d.model.iter().for_each(|e| take(e));
    } else {
        handles.iter().filter_map(|h| d.entity(*h)).for_each(|e| take(e));
    }
    if out.len() > MAX_POINTS {
        return Err(bad("tin", format!("more than {MAX_POINTS} points: select fewer")));
    }
    Ok(out)
}

/// Delaunay triangles of the points (the first of points sharing a position wins).
fn triangulate(points: &[Vec3], max_edge: Option<f64>) -> Result<Vec<[Vec3; 3]>> {
    if points.len() < 3 {
        return Err(bad("tin", "at least 3 points are needed (import survey points first)"));
    }
    let verts: Vec<Vtx> = points.iter().map(|p| Vtx { x: p.x, y: p.y, z: p.z }).collect();
    let t = DelaunayTriangulation::<Vtx>::bulk_load(verts).map_err(|e| bad("tin", format!("cannot triangulate: {e}")))?;
    let mut out = Vec::new();
    for f in t.inner_faces() {
        let [a, b, c] = f.vertices().map(|v| {
            let d = v.data();
            Vec3::new(d.x, d.y, d.z)
        });
        if let Some(m) = max_edge {
            let long = [(a, b), (b, c), (c, a)].iter().any(|(p, q)| (p.x - q.x).hypot(p.y - q.y) > m);
            if long {
                continue;
            }
        }
        if out.len() >= MAX_TRIANGLES {
            return Err(bad("tin", format!("more than {MAX_TRIANGLES} triangles")));
        }
        out.push([a, b, c]);
    }
    if out.is_empty() {
        return Err(bad("tin", "no triangles: the points are in a line, or `maxEdge` is smaller than their spacing"));
    }
    Ok(out)
}

/// The surface's triangles: the given 3DFACE objects, those on the TIN layer, or a TIN built
/// from the drawing's points.
pub(super) fn surface(s: &Session, handles: &[Handle], layer: &str) -> Result<Vec<[Vec3; 3]>> {
    let d = s.doc()?;
    let mut tris = Vec::new();
    let mut push = |e: &cadcraft_doc::Entity| {
        if let EntityKind::Face3d(f) = &e.kind {
            let c = f.corners;
            if c.iter().all(finite3) {
                tris.push([c[0], c[1], c[2]]);
                if c[3] != c[2] {
                    tris.push([c[0], c[2], c[3]]);
                }
            }
        }
    };
    if handles.is_empty() {
        d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(layer)).for_each(|e| push(e));
    } else {
        handles.iter().filter_map(|h| d.entity(*h)).for_each(|e| push(e));
    }
    if tris.len() > MAX_TRIANGLES {
        return Err(bad("contour", format!("more than {MAX_TRIANGLES} triangles")));
    }
    if tris.is_empty() {
        let pts = gather_points(s, &[])?;
        return triangulate(&pts, None).map_err(|_| bad("contour", "no surface: create a TIN first, or import survey points"));
    }
    Ok(tris)
}

pub(super) fn handles_param(p: &Value) -> Vec<Handle> {
    p.get("handles").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().and_then(Handle::parse_hex)).collect()).unwrap_or_default()
}

pub(super) fn layer_name(cmd: &str, raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().count() > 64 || raw.chars().any(|c| c.is_control() || "<>/\\\":;?*|=`".contains(c)) {
        return Err(bad(cmd, "layer names are 1 to 64 characters without < > / \\ \" : ; ? * | = `"));
    }
    Ok(raw.to_string())
}

fn run_tin(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = handles_param(p);
    let max_edge = size_param("tin", p, "maxEdge", false)?;
    let layer = layer_name("tin", str_param(p, "layer").unwrap_or("TIN"))?;
    let points = gather_points(s, &handles)?;
    let tris = triangulate(&points, max_edge)?;
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for t in &tris {
        for v in t {
            zmin = zmin.min(v.z);
            zmax = zmax.max(v.z);
        }
    }
    let space = s.space();
    let com = Common { layer: layer.clone(), color: Color::Index(8), ..s.current_common() };
    let d = s.doc_mut()?;
    d.ensure_layer(&layer);
    for [a, b, c] in &tris {
        d.add(&space, com.clone(), EntityKind::Face3d(Face3d { corners: [*a, *b, *c, *c], hidden_edges: 0 }))?;
    }
    let flat = if zmax - zmin < 1e-9 { " (all points are at the same elevation)" } else { "" };
    let msg = format!("TIN: {} triangles from {} points, elevation {zmin:.3} to {zmax:.3}{flat}", tris.len(), points.len());
    s.echo(msg.clone());
    Ok(json!({ "triangles": tris.len(), "points": points.len(), "zMin": zmin, "zMax": zmax, "layer": layer, "message": msg }))
}

/// One contour: a chain of points at one elevation.
#[derive(Debug, Clone, PartialEq)]
struct ContourLine {
    level: i64,
    pts: Vec<Vec2>,
    closed: bool,
}

type EdgeKey = (u32, u32);

/// Contours through triangles at `base + k * interval`. A vertex exactly at a level counts as
/// above it, so neighbouring triangles agree on where the line crosses their shared edge.
fn contour_lines(tris: &[[Vec3; 3]], base: f64, interval: f64) -> Result<Vec<ContourLine>> {
    // Shared vertices, so edges can be matched between triangles.
    let mut ids: HashMap<(u64, u64), u32> = HashMap::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut mesh: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
    for t in tris {
        let mut m = [0u32; 3];
        for (slot, v) in m.iter_mut().zip(t) {
            let key = (v.x.to_bits(), v.y.to_bits());
            *slot = match ids.get(&key) {
                Some(i) => *i,
                None => {
                    let i = u32::try_from(verts.len()).map_err(|_| bad("contour", "too many vertices"))?;
                    verts.push(*v);
                    ids.insert(key, i);
                    i
                }
            };
        }
        mesh.push(m);
    }
    let z = |i: u32| verts.get(i as usize).map(|v| v.z).unwrap_or(0.0);
    let mut per_level: HashMap<i64, Vec<(EdgeKey, EdgeKey)>> = HashMap::new();
    let mut total = 0usize;
    for m in &mesh {
        let zs = m.map(z);
        let (lo, hi) = (zs.iter().cloned().fold(f64::INFINITY, f64::min), zs.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
        if hi <= lo {
            continue;
        }
        let mut k = ((lo - base) / interval).floor() as i64;
        while base + k as f64 * interval <= lo {
            k += 1;
        }
        loop {
            let level = base + k as f64 * interval;
            if level > hi {
                break;
            }
            let mut crossed: Vec<EdgeKey> = Vec::with_capacity(2);
            for (a, b) in [(0usize, 1usize), (1, 2), (2, 0)] {
                let (ia, ib) = (m[a], m[b]);
                if (z(ia) >= level) != (z(ib) >= level) {
                    crossed.push((ia.min(ib), ia.max(ib)));
                }
            }
            if let [e1, e2] = crossed[..] {
                per_level.entry(k).or_default().push((e1, e2));
                total += 1;
                if total > MAX_SEGMENTS {
                    return Err(bad("contour", "too many contour segments: use a larger `interval`"));
                }
            }
            k += 1;
        }
    }

    let point_on = |e: EdgeKey, level: f64| -> Vec2 {
        let (Some(a), Some(b)) = (verts.get(e.0 as usize), verts.get(e.1 as usize)) else { return Vec2::ZERO };
        let t = if (b.z - a.z).abs() > 0.0 { ((level - a.z) / (b.z - a.z)).clamp(0.0, 1.0) } else { 0.5 };
        Vec2::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y))
    };

    let mut keys: Vec<i64> = per_level.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for k in keys {
        let Some(segs) = per_level.get(&k) else { continue };
        let level = base + k as f64 * interval;
        let mut at: HashMap<EdgeKey, Vec<usize>> = HashMap::new();
        for (i, (a, b)) in segs.iter().enumerate() {
            at.entry(*a).or_default().push(i);
            at.entry(*b).or_default().push(i);
        }
        let mut used = vec![false; segs.len()];
        // Open lines start at an edge only one segment touches; what is left are closed rings.
        let mut starts: Vec<(usize, EdgeKey)> = Vec::new();
        for (e, v) in &at {
            if let [i] = v[..] {
                starts.push((i, *e));
            }
        }
        starts.sort_unstable();
        let ring_starts: Vec<(usize, EdgeKey)> = segs.iter().enumerate().map(|(i, (a, _))| (i, *a)).collect();
        for (pass_closed, list) in [(false, starts), (true, ring_starts)] {
            for (i0, e0) in list {
                if used.get(i0).copied().unwrap_or(true) {
                    continue;
                }
                let mut chain = vec![e0];
                let mut cur = i0;
                let mut enter = e0;
                loop {
                    if let Some(u) = used.get_mut(cur) {
                        *u = true;
                    }
                    let Some((a, b)) = segs.get(cur) else { break };
                    let next_edge = if *a == enter { *b } else { *a };
                    chain.push(next_edge);
                    let next_seg = at.get(&next_edge).and_then(|v| v.iter().copied().find(|j| !used.get(*j).copied().unwrap_or(true)));
                    match next_seg {
                        Some(j) => {
                            cur = j;
                            enter = next_edge;
                        }
                        None => break,
                    }
                }
                let closed = pass_closed && chain.len() > 3 && chain.first() == chain.last();
                if closed {
                    chain.pop();
                }
                let mut pts: Vec<Vec2> = Vec::with_capacity(chain.len());
                for e in chain {
                    let q = point_on(e, level);
                    if pts.last().is_none_or(|l| (l.x - q.x).hypot(l.y - q.y) > 1e-12) {
                        pts.push(q);
                    }
                }
                if pts.len() >= 2 {
                    out.push(ContourLine { level: k, pts, closed });
                }
            }
        }
    }
    Ok(out)
}

/// A round interval (1, 2 or 5 × a power of ten) giving about ten levels over `range`.
fn nice_interval(range: f64) -> f64 {
    let raw = (range / 10.0).max(f64::MIN_POSITIVE);
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

/// Rounds the corners of a polyline by cutting each one off (Chaikin), `rounds` times. An open
/// line keeps its end points; a ring stays closed. The curve stays inside the original lines'
/// outline, so neighbouring contours do not cross each other.
fn smooth_line(pts: &[Vec2], closed: bool, rounds: u32) -> Vec<Vec2> {
    let mut cur = pts.to_vec();
    for _ in 0..rounds {
        let n = cur.len();
        if !(3..=200_000).contains(&n) {
            break;
        }
        let segs = if closed { n } else { n - 1 };
        let mut out = Vec::with_capacity(segs * 2 + 2);
        if !closed && let Some(first) = cur.first() {
            out.push(*first);
        }
        for i in 0..segs {
            let (Some(a), Some(b)) = (cur.get(i), cur.get((i + 1) % n)) else { break };
            if closed || i > 0 {
                out.push(Vec2::new(0.75 * a.x + 0.25 * b.x, 0.75 * a.y + 0.25 * b.y));
            }
            if closed || i + 1 < segs {
                out.push(Vec2::new(0.25 * a.x + 0.75 * b.x, 0.25 * a.y + 0.75 * b.y));
            }
        }
        if !closed && let Some(last) = cur.last() {
            out.push(*last);
        }
        cur = out;
    }
    cur
}

fn polyline_length(pts: &[Vec2], closed: bool) -> f64 {
    let mut len: f64 = pts.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
    if closed && let (Some(a), Some(b)) = (pts.first(), pts.last()) {
        len += (a.x - b.x).hypot(a.y - b.y);
    }
    len
}

/// The point half way along the polyline and the direction there.
fn midpoint(pts: &[Vec2], closed: bool) -> Option<(Vec2, f64)> {
    let total = polyline_length(pts, closed);
    let mut walk = 0.0;
    let n = pts.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (a, b) = (*pts.get(i)?, *pts.get((i + 1) % n)?);
        let l = (b.x - a.x).hypot(b.y - a.y);
        if walk + l >= total / 2.0 && l > 0.0 {
            let t = (total / 2.0 - walk) / l;
            let mut ang = (b.y - a.y).atan2(b.x - a.x);
            // Keep the text readable (never upside down).
            if ang > std::f64::consts::FRAC_PI_2 {
                ang -= std::f64::consts::PI;
            } else if ang <= -std::f64::consts::FRAC_PI_2 {
                ang += std::f64::consts::PI;
            }
            return Some((Vec2::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)), ang));
        }
        walk += l;
    }
    None
}

fn run_contour(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = handles_param(p);
    let tris = surface(s, &handles, "TIN")?;
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for t in &tris {
        for v in t {
            zmin = zmin.min(v.z);
            zmax = zmax.max(v.z);
        }
    }
    if zmax - zmin <= 1e-9 {
        return Err(bad("contour", "the surface is flat: its points have no elevation differences"));
    }
    let interval = size_param("contour", p, "interval", false)?.unwrap_or_else(|| nice_interval(zmax - zmin));
    let base = f64_or(p, "base", 0.0);
    if !finite_in(interval) || !finite_in(base) {
        return Err(bad("contour", "`interval` and `base` must be finite"));
    }
    let major_every = match p.get("majorEvery") {
        None | Some(Value::Null) => 5,
        Some(v) => v.as_u64().filter(|n| (1..=1000).contains(n)).ok_or_else(|| bad("contour", "`majorEvery` must be 1 to 1000"))?,
    } as i64;
    let smooth = match p.get("smooth") {
        None | Some(Value::Null) => 2,
        Some(v) => v.as_u64().filter(|n| *n <= 4).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("contour", "`smooth` must be 0 to 4"))?,
    };
    let labels = bool_or(p, "labels", true);
    let decimals = match p.get("decimals") {
        None | Some(Value::Null) => {
            if interval >= 1.0 {
                0
            } else if interval >= 0.1 {
                1
            } else {
                2
            }
        }
        Some(v) => v.as_u64().filter(|d| *d <= 6).and_then(|d| usize::try_from(d).ok()).ok_or_else(|| bad("contour", "`decimals` must be 0 to 6"))?,
    };
    let extent = tris.iter().flatten().fold(cadcraft_geom::Bounds2::EMPTY, |mut b, v| {
        b.add(Vec2::new(v.x, v.y));
        b
    });
    let text_h =
        size_param("contour", p, "textHeight", false)?.unwrap_or_else(|| (extent.width().max(extent.height()) / 150.0).max(f64::MIN_POSITIVE));
    let prefix = layer_name("contour", str_param(p, "layerPrefix").unwrap_or("CONTOUR"))?;

    let lines = contour_lines(&tris, base, interval)?;
    let (l_minor, l_major, l_text) = (format!("{prefix}-MINOR"), format!("{prefix}-MAJOR"), format!("{prefix}-LABEL"));
    let space = s.space();
    let basec = s.current_common();
    {
        let d = s.doc_mut()?;
        for l in [&l_minor, &l_major, &l_text] {
            d.ensure_layer(l);
        }
    }
    let minor = Common { layer: l_minor.clone(), color: Color::Index(30), ..basec.clone() };
    let major = Common { layer: l_major.clone(), color: Color::Index(10), lineweight: Lineweight::Mm100(35), ..basec.clone() };
    let text = Common { layer: l_text.clone(), color: Color::Index(10), ..basec };
    let (mut n_major, mut n_minor, mut n_labels) = (0usize, 0usize, 0usize);
    for raw in &lines {
        let line = &ContourLine { pts: smooth_line(&raw.pts, raw.closed, smooth), ..raw.clone() };
        let level = base + line.level as f64 * interval;
        let is_major = line.level.rem_euclid(major_every) == 0;
        let pl = EntityKind::LwPolyline(LwPolyline {
            vertices: line.pts.iter().copied().map(PolyVertex::new).collect(),
            closed: line.closed,
            const_width: 0.0,
            elevation: level,
            plinegen: false,
        });
        s.doc_mut()?.add(&space, if is_major { major.clone() } else { minor.clone() }, pl)?;
        if is_major {
            n_major += 1;
        } else {
            n_minor += 1;
        }
        if is_major
            && labels
            && polyline_length(&line.pts, line.closed) >= text_h * 10.0
            && let Some((at, ang)) = midpoint(&line.pts, line.closed)
        {
            let t = make_text(s, at, &format!("{level:.decimals$}"), text_h, ang, (HAlign::Center, VAlign::Middle));
            s.doc_mut()?.add(&space, text.clone(), t)?;
            n_labels += 1;
        }
    }
    let msg = format!("contours every {interval} from {zmin:.2} to {zmax:.2}: {n_minor} minor and {n_major} major lines, {n_labels} labels");
    s.echo(msg.clone());
    Ok(json!({
        "interval": interval, "base": base, "zMin": zmin, "zMax": zmax, "minor": n_minor, "major": n_major,
        "labels": n_labels, "triangles": tris.len(), "layers": [l_minor, l_major, l_text], "message": msg,
    }))
}

pub(super) fn finite_in(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

fn number(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// TIN at the command line: which points, then the longest triangle side.
#[derive(Default)]
struct TinM {
    handles: Option<Vec<Handle>>,
}

impl Interactive for TinM {
    fn name(&self) -> &'static str {
        "TIN"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let sel = s.selection();
        if !sel.is_empty() {
            self.handles = Some(sel);
            s.set_selection(Vec::new());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.handles.is_none() {
            Prompt::new("Select the survey points (Enter: every point in the drawing)", Accept::SELECT)
        } else {
            Prompt::new("Longest triangle side to keep, in drawing units (Enter: keep all)", Accept::TEXT)
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(handles) = self.handles.clone() else {
            match i {
                Input::Pick(hs) => self.handles = Some(hs),
                Input::Enter => self.handles = Some(Vec::new()),
                Input::Cancel => return Ok(Step::Cancel),
                _ => {
                    s.echo("Select points, or press Enter for all.".to_string());
                }
            }
            return Ok(Step::Continue);
        };
        let mut params = json!({ "handles": handles.iter().map(|h| h.hex()).collect::<Vec<_>>() });
        match i {
            Input::Text(t) if !t.trim().is_empty() => match number(&t) {
                Some(v) => params["maxEdge"] = json!(v),
                None => {
                    s.echo("Enter a positive number.".to_string());
                    return Ok(Step::Continue);
                }
            },
            Input::Cancel => return Ok(Step::Cancel),
            _ => {}
        }
        run_tin(s, &params)?;
        Ok(Step::Done)
    }
}

/// CONTOUR at the command line: interval, then how often a major contour is drawn.
#[derive(Default)]
struct ContourM {
    interval: Option<f64>,
    auto: Option<f64>,
    major: Option<u64>,
    /// 0 interval, 1 major spacing, 2 smoothing.
    step: u8,
}

impl Interactive for ContourM {
    fn name(&self) -> &'static str {
        "CONTOUR"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let tris = surface(s, &[], "TIN")?;
        let (lo, hi) = tris.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), v| (l.min(v.z), h.max(v.z)));
        if hi - lo > 1e-9 {
            self.auto = Some(nice_interval(hi - lo));
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let mut p = match self.step {
            0 => {
                let mut p = Prompt::new("Contour interval in drawing units", Accept::TEXT);
                p.default = self.auto.map(|v| v.to_string());
                return p;
            }
            1 => Prompt::new("Draw a major (thick, labelled) contour every how many contours", Accept::TEXT),
            _ => Prompt::new("Smoothing of the curves, 0 (straight lines) to 4", Accept::TEXT),
        };
        p.default = Some(if self.step == 1 { "5" } else { "2" }.into());
        p
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let text = match i {
            Input::Text(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => return Ok(Step::Cancel),
        };
        match self.step {
            0 => {
                if !text.is_empty() {
                    match number(&text) {
                        Some(v) => self.interval = Some(v),
                        None => {
                            s.echo("Enter a positive number.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                self.step = 1;
                Ok(Step::Continue)
            }
            1 => {
                if !text.is_empty() {
                    match text.parse::<u64>() {
                        Ok(n) if (1..=1000).contains(&n) => self.major = Some(n),
                        _ => {
                            s.echo("Enter a whole number from 1 to 1000.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                self.step = 2;
                Ok(Step::Continue)
            }
            _ => {
                let mut params = json!({});
                if let Some(v) = self.interval {
                    params["interval"] = json!(v);
                }
                if let Some(n) = self.major {
                    params["majorEvery"] = json!(n);
                }
                if !text.is_empty() {
                    match text.parse::<u64>() {
                        Ok(n) if n <= 4 => params["smooth"] = json!(n),
                        _ => {
                            s.echo("Enter a whole number from 0 to 4.".to_string());
                            return Ok(Step::Continue);
                        }
                    }
                }
                run_contour(s, &params)?;
                Ok(Step::Done)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: (f64, f64, f64), b: (f64, f64, f64), c: (f64, f64, f64)) -> [Vec3; 3] {
        [Vec3::new(a.0, a.1, a.2), Vec3::new(b.0, b.1, b.2), Vec3::new(c.0, c.1, c.2)]
    }

    #[test]
    fn a_sloping_plane_gives_straight_contours() {
        // z = x over a square split in two triangles.
        let t = [tri((0.0, 0.0, 0.0), (10.0, 0.0, 10.0), (10.0, 10.0, 10.0)), tri((0.0, 0.0, 0.0), (10.0, 10.0, 10.0), (0.0, 10.0, 0.0))];
        let lines = contour_lines(&t, 0.0, 3.0).unwrap();
        assert_eq!(lines.len(), 3, "z = 3, 6, 9");
        for l in &lines {
            let x = l.level as f64 * 3.0;
            assert!(l.pts.iter().all(|p| (p.x - x).abs() < 1e-9), "{l:?}");
            assert_eq!(l.pts.len(), 3, "one line through both triangles, with a vertex on the diagonal: {l:?}");
            assert!(!l.closed);
        }
    }

    #[test]
    fn a_hill_gives_a_closed_ring() {
        // A pyramid: apex 10 m high over a 20 m square base.
        let apex = (0.0, 0.0, 10.0);
        let c = [(-10.0, -10.0, 0.0), (10.0, -10.0, 0.0), (10.0, 10.0, 0.0), (-10.0, 10.0, 0.0)];
        let t: Vec<[Vec3; 3]> = (0..4).map(|i| tri(apex, c[i], c[(i + 1) % 4])).collect();
        let lines = contour_lines(&t, 0.0, 5.0).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed, "{:?}", lines[0]);
        assert!(lines[0].pts.iter().all(|p| (p.x.abs().max(p.y.abs()) - 5.0).abs() < 1e-9));
    }

    #[test]
    fn levels_at_a_vertex_do_not_break_the_line() {
        // The plane z = x has vertices exactly on the level 10.
        let t = [tri((0.0, 0.0, 0.0), (10.0, 0.0, 10.0), (0.0, 10.0, 0.0)), tri((10.0, 0.0, 10.0), (10.0, 10.0, 10.0), (0.0, 10.0, 0.0))];
        let lines = contour_lines(&t, 0.0, 5.0).unwrap();
        assert!(lines.iter().any(|l| l.level == 1), "{lines:?}");
    }

    #[test]
    fn smoothing_rounds_corners_and_keeps_the_ends() {
        let line = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let s = smooth_line(&line, false, 2);
        assert!(s.len() > line.len());
        assert_eq!(s.first(), line.first());
        assert_eq!(s.last(), line.last());
        // The corner point (10, 0) is cut off.
        assert!(s.iter().all(|p| (p.x - 10.0).abs() > 1e-9 || p.y.abs() > 1e-9));
        let ring = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        assert_eq!(smooth_line(&ring, true, 1).len(), 8);
        assert_eq!(smooth_line(&ring, true, 0), ring.to_vec());
    }

    #[test]
    fn intervals_are_round() {
        assert_eq!(nice_interval(10.0), 1.0);
        assert_eq!(nice_interval(23.0), 2.0);
        assert_eq!(nice_interval(0.8), 0.1);
    }
}
