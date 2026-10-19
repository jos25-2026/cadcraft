//! EARTHWORK: cut and fill volumes between the ground surface (the TIN) and a flat design
//! elevation, optionally limited to a closed boundary polyline. Without an elevation the
//! command finds the one that balances cut against fill. Volumes are exact for the triangulated
//! surface: each triangle is split where it crosses the design plane.

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, HAlign, Handle, VAlign};
use cadcraft_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::terrain::{finite_in, handles_param, layer_name, surface};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most boundary corners accepted (the ear clipping is quadratic or worse).
const MAX_BOUNDARY: usize = 1000;
/// Most surface pieces kept after clipping to the boundary.
const MAX_PIECES: usize = 6_000_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("earthwork", "Earthwork Volumes...", run_earthwork)
            .alias(&["cutfill", "volume", "volumes"])
            .menu(&["Tools", "Terrain", "Earthwork Volumes..."])
            .params(
                "{elevation?: design elevation (default: the elevation that balances cut and fill), boundary?: handle of a closed \
                 polyline limiting the area (default: the whole surface), handles?: [3DFACE triangles] (default: the TIN layer, else \
                 a TIN is built from the points), label?: true (write the result into the drawing), at?: [x, y] for the label, \
                 textHeight?, layer?: \"EARTHWORK\"} → {cut, fill, net, area, elevation}",
            )
            .interactive(|_| Ok(Box::new(EarthM::default()))),
    ]
}

fn cross(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn area2(poly: &[Vec2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum()
}

fn in_triangle(p: Vec2, t: &[Vec2; 3]) -> bool {
    let (a, b, c) = (cross(t[0], t[1], p), cross(t[1], t[2], p), cross(t[2], t[0], p));
    a >= 0.0 && b >= 0.0 && c >= 0.0
}

/// A simple polygon cut into counter-clockwise triangles (ear clipping); `None` when it is
/// degenerate or crosses itself.
fn ear_clip(poly: &[Vec2]) -> Option<Vec<[Vec2; 3]>> {
    let mut pts: Vec<Vec2> = Vec::with_capacity(poly.len());
    for p in poly {
        if pts.last().is_none_or(|q| (q.x - p.x).abs() > 1e-12 || (q.y - p.y).abs() > 1e-12) {
            pts.push(*p);
        }
    }
    if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (a.x - b.x).abs() <= 1e-12 && (a.y - b.y).abs() <= 1e-12) {
        pts.pop();
    }
    if pts.len() < 3 {
        return None;
    }
    let a2 = area2(&pts);
    if a2.abs() < 1e-12 {
        return None;
    }
    if a2 < 0.0 {
        pts.reverse();
    }
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0usize;
    while idx.len() > 3 {
        guard += 1;
        if guard > pts.len() * pts.len() + 10 {
            return None;
        }
        let n = idx.len();
        let mut cut_at = None;
        for k in 0..n {
            let (a, b, c) = (pts[*idx.get((k + n - 1) % n)?], pts[*idx.get(k)?], pts[*idx.get((k + 1) % n)?]);
            let cr = cross(a, b, c);
            if cr.abs() < 1e-12 {
                // A straight-through corner adds no area.
                cut_at = Some((k, false));
                break;
            }
            if cr < 0.0 {
                continue;
            }
            let t = [a, b, c];
            let blocked = idx.iter().any(|&j| {
                let q = pts[j];
                q != a && q != b && q != c && in_triangle(q, &t)
            });
            if !blocked {
                cut_at = Some((k, true));
                break;
            }
        }
        let (k, emit) = cut_at?;
        if emit {
            out.push([pts[*idx.get((k + n - 1) % n)?], pts[*idx.get(k)?], pts[*idx.get((k + 1) % n)?]]);
        }
        idx.remove(k);
    }
    let t = [pts[*idx.first()?], pts[*idx.get(1)?], pts[*idx.get(2)?]];
    if cross(t[0], t[1], t[2]).abs() > 1e-12 {
        out.push(t);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// The part of `subject` inside the convex counter-clockwise `clip` (Sutherland–Hodgman).
fn clip_convex(subject: &[Vec2], clip: &[Vec2; 3]) -> Vec<Vec2> {
    let mut poly = subject.to_vec();
    for i in 0..3 {
        let (a, b) = (clip[i], clip[(i + 1) % 3]);
        let input = std::mem::take(&mut poly);
        let n = input.len();
        for j in 0..n {
            let (p, q) = (input[j], input[(j + 1) % n]);
            let (sp, sq) = (cross(a, b, p), cross(a, b, q));
            if sp >= 0.0 {
                poly.push(p);
            }
            if (sp > 0.0 && sq < 0.0) || (sp < 0.0 && sq > 0.0) {
                let t = sp / (sp - sq);
                poly.push(Vec2::new(p.x + t * (q.x - p.x), p.y + t * (q.y - p.y)));
            }
        }
        if poly.is_empty() {
            break;
        }
    }
    poly
}

/// Height of the plane through triangle `t` at (x, y); `None` for a vertical or empty triangle.
fn z_at(t: &[Vec3; 3], x: f64, y: f64) -> Option<f64> {
    let det = (t[1].y - t[2].y) * (t[0].x - t[2].x) + (t[2].x - t[1].x) * (t[0].y - t[2].y);
    if det.abs() < 1e-18 {
        return None;
    }
    let l0 = ((t[1].y - t[2].y) * (x - t[2].x) + (t[2].x - t[1].x) * (y - t[2].y)) / det;
    let l1 = ((t[2].y - t[0].y) * (x - t[2].x) + (t[0].x - t[2].x) * (y - t[2].y)) / det;
    Some(l0 * t[0].z + l1 * t[1].z + (1.0 - l0 - l1) * t[2].z)
}

/// The surface triangles restricted to the boundary (all of them when there is none).
fn limit_to(tris: &[[Vec3; 3]], boundary: Option<&[Vec2]>) -> Result<Vec<[Vec3; 3]>> {
    let Some(b) = boundary else { return Ok(tris.to_vec()) };
    let ears = ear_clip(b).ok_or_else(|| bad("earthwork", "the boundary must be a closed polyline that does not cross itself"))?;
    let bbox: Vec<(f64, f64, f64, f64)> = ears
        .iter()
        .map(|t| {
            let xs = t.map(|p| p.x);
            let ys = t.map(|p| p.y);
            (
                xs.iter().copied().fold(f64::INFINITY, f64::min),
                ys.iter().copied().fold(f64::INFINITY, f64::min),
                xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            )
        })
        .collect();
    let mut out = Vec::new();
    for t in tris {
        let (x0, y0) = (t.iter().map(|v| v.x).fold(f64::INFINITY, f64::min), t.iter().map(|v| v.y).fold(f64::INFINITY, f64::min));
        let (x1, y1) = (t.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max), t.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max));
        let mut sub = [Vec2::new(t[0].x, t[0].y), Vec2::new(t[1].x, t[1].y), Vec2::new(t[2].x, t[2].y)];
        if cross(sub[0], sub[1], sub[2]) < 0.0 {
            sub.swap(1, 2);
        }
        for (ear, bb) in ears.iter().zip(&bbox) {
            if bb.2 < x0 || bb.0 > x1 || bb.3 < y0 || bb.1 > y1 {
                continue;
            }
            let poly = clip_convex(&sub, ear);
            if poly.len() < 3 {
                continue;
            }
            let mut v = Vec::with_capacity(poly.len());
            for p in &poly {
                match z_at(t, p.x, p.y) {
                    Some(z) => v.push(Vec3::new(p.x, p.y, z)),
                    None => break,
                }
            }
            if v.len() != poly.len() {
                continue;
            }
            for k in 1..v.len() - 1 {
                if out.len() >= MAX_PIECES {
                    return Err(bad("earthwork", "the surface is too detailed for this boundary"));
                }
                out.push([v[0], v[k], v[k + 1]]);
            }
        }
    }
    Ok(out)
}

fn plan_area(t: &[Vec3; 3]) -> f64 {
    0.5 * ((t[1].x - t[0].x) * (t[2].y - t[0].y) - (t[2].x - t[0].x) * (t[1].y - t[0].y)).abs()
}

/// (cut, fill) between the pieces and the plane at `h`: cut where the ground is above it.
fn volumes(pieces: &[[Vec3; 3]], h: f64) -> (f64, f64) {
    let (mut cut, mut fill) = (0.0, 0.0);
    for t in pieces {
        let a = plan_area(t);
        let d = [t[0].z - h, t[1].z - h, t[2].z - h];
        if d.iter().all(|v| *v >= 0.0) {
            cut += a * (d[0] + d[1] + d[2]) / 3.0;
        } else if d.iter().all(|v| *v <= 0.0) {
            fill -= a * (d[0] + d[1] + d[2]) / 3.0;
        } else {
            // One corner (the apex) is alone on its side; the plane cuts a small pyramid off it.
            let Some(i) = (0..3).find(|&i| d[i] != 0.0 && d[i] * d[(i + 1) % 3] <= 0.0 && d[i] * d[(i + 2) % 3] <= 0.0) else {
                continue;
            };
            let (d0, d1, d2) = (d[i], d[(i + 1) % 3], d[(i + 2) % 3]);
            let (t1, t2) = (d0 / (d0 - d1), d0 / (d0 - d2));
            let apex = a * t1 * t2 * d0 / 3.0;
            let rest = a * (d0 + d1 + d2) / 3.0 - apex;
            if d0 > 0.0 {
                cut += apex;
                fill -= rest;
            } else {
                fill -= apex;
                cut += rest;
            }
        }
    }
    (cut, fill)
}

/// The design elevation at which cut equals fill.
fn balanced(pieces: &[[Vec3; 3]], lo: f64, hi: f64) -> f64 {
    let (mut lo, mut hi) = (lo, hi);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        let (c, f) = volumes(pieces, mid);
        if c > f {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The corners of a closed polyline.
fn boundary_of(s: &Session, h: Handle) -> Result<Vec<Vec2>> {
    let d = s.doc()?;
    match d.entity(h).map(|e| &e.kind) {
        Some(EntityKind::LwPolyline(pl)) if pl.closed => {
            if pl.vertices.len() > MAX_BOUNDARY {
                return Err(bad("earthwork", format!("the boundary has more than {MAX_BOUNDARY} corners")));
            }
            let pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
            if pts.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
                return Err(bad("earthwork", "the boundary has invalid coordinates"));
            }
            Ok(pts)
        }
        Some(_) => Err(bad("earthwork", "the boundary must be a closed polyline (draw a rectangle or a closed PLINE)")),
        None => Err(bad("earthwork", "boundary: no such object")),
    }
}

fn run_earthwork(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = handles_param(p);
    let tris = surface(s, &handles, "TIN")?;
    let boundary = match str_param(p, "boundary") {
        Some(t) => Some(boundary_of(s, Handle::parse_hex(t).ok_or_else(|| bad("earthwork", "`boundary` is not a handle"))?)?),
        None => None,
    };
    let pieces = limit_to(&tris, boundary.as_deref())?;
    if pieces.is_empty() {
        return Err(bad("earthwork", "the boundary does not overlap the surface"));
    }
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut ext = cadcraft_geom::Bounds2::EMPTY;
    let mut area = 0.0;
    for t in &pieces {
        area += plan_area(t);
        for v in t {
            zmin = zmin.min(v.z);
            zmax = zmax.max(v.z);
            ext.add(Vec2::new(v.x, v.y));
        }
    }
    let given = match p.get("elevation") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|e| finite_in(*e)).ok_or_else(|| bad("earthwork", "`elevation` must be a number"))?),
    };
    let level = given.unwrap_or_else(|| balanced(&pieces, zmin, zmax));
    let (cut, fill) = volumes(&pieces, level);
    let net = cut - fill;
    let how = if given.is_some() { "" } else { " (balanced)" };
    let msg = format!(
        "Earthwork at elevation {level:.3}{how}: area {area:.2}, CUT {cut:.2}, FILL {fill:.2}, net {net:+.2} ({})",
        if net >= 0.0 { "surplus to remove" } else { "to import" }
    );
    s.echo(msg.clone());
    let mut labelled = false;
    if bool_or(p, "label", true) {
        let layer = layer_name("earthwork", str_param(p, "layer").unwrap_or("EARTHWORK"))?;
        let th = size_param("earthwork", p, "textHeight", false)?.unwrap_or_else(|| (ext.width().max(ext.height()) / 60.0).max(f64::MIN_POSITIVE));
        let at = point_param(p, "at").unwrap_or_else(|| Vec2::new(ext.min.x, ext.min.y - 2.0 * th));
        let space = s.space();
        let com = Common { layer: layer.clone(), color: Color::Index(2), ..s.current_common() };
        s.doc_mut()?.ensure_layer(&layer);
        let lines = [
            format!("EARTHWORK  design elevation {level:.2}{how}"),
            format!("AREA = {area:.2}"),
            format!("CUT  = {cut:.2}"),
            format!("FILL = {fill:.2}"),
            format!("NET  = {net:+.2}"),
        ];
        for (i, l) in lines.iter().enumerate() {
            let y = at.y - 1.6 * th * i as f64;
            let t = make_text(s, Vec2::new(at.x, y), l, th, 0.0, (HAlign::Left, VAlign::Middle));
            s.doc_mut()?.add(&space, com.clone(), t)?;
        }
        labelled = true;
    }
    Ok(
        json!({ "elevation": level, "balanced": given.is_none(), "cut": cut, "fill": fill, "net": net, "area": area, "triangles": pieces.len(), "labelled": labelled, "message": msg }),
    )
}

/// EARTHWORK at the command line: the boundary, then the design elevation.
#[derive(Default)]
struct EarthM {
    boundary: Option<Handle>,
    /// 0 boundary, 1 elevation.
    step: u8,
}

fn closed_polyline(s: &Session, hs: &[Handle]) -> Option<Handle> {
    let d = s.doc().ok()?;
    hs.iter().copied().find(|h| matches!(d.entity(*h).map(|e| &e.kind), Some(EntityKind::LwPolyline(pl)) if pl.closed))
}

impl Interactive for EarthM {
    fn name(&self) -> &'static str {
        "EARTHWORK"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        surface(s, &[], "TIN")?;
        let sel = s.selection();
        if let Some(h) = closed_polyline(s, &sel) {
            self.boundary = Some(h);
            self.step = 1;
            s.set_selection(Vec::new());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.step == 0 {
            Prompt::new("Select the boundary (a closed polyline or rectangle; Enter: the whole surface)", Accept::SELECT)
        } else {
            let mut p = Prompt::new("Design elevation (Enter: the elevation where cut equals fill)", Accept::TEXT);
            p.default = None;
            p
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.step == 0 {
            match i {
                Input::Pick(hs) => match closed_polyline(s, &hs) {
                    Some(h) => {
                        self.boundary = Some(h);
                        self.step = 1;
                    }
                    None => s.echo("Pick a closed polyline or a rectangle.".to_string()),
                },
                Input::Enter => self.step = 1,
                Input::Cancel => return Ok(Step::Cancel),
                _ => s.echo("Select the boundary, or press Enter for the whole surface.".to_string()),
            }
            return Ok(Step::Continue);
        }
        let mut params = json!({});
        if let Some(h) = self.boundary {
            params["boundary"] = json!(h.hex());
        }
        match i {
            Input::Text(t) if !t.trim().is_empty() => match t.trim().parse::<f64>() {
                Ok(v) if finite_in(v) => params["elevation"] = json!(v),
                _ => {
                    s.echo("Enter a number, or press Enter to balance cut and fill.".to_string());
                    return Ok(Step::Continue);
                }
            },
            Input::Cancel => return Ok(Step::Cancel),
            _ => {}
        }
        run_earthwork(s, &params)?;
        Ok(Step::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(z: impl Fn(f64, f64) -> f64, size: f64) -> Vec<[Vec3; 3]> {
        let v = |x: f64, y: f64| Vec3::new(x, y, z(x, y));
        vec![[v(0.0, 0.0), v(size, 0.0), v(size, size)], [v(0.0, 0.0), v(size, size), v(0.0, size)]]
    }

    #[test]
    fn a_slope_cut_and_fill_balance_at_its_middle() {
        let t = square(|x, _| x, 10.0);
        let (c, f) = volumes(&t, 5.0);
        assert!((c - 125.0).abs() < 1e-9 && (f - 125.0).abs() < 1e-9, "{c} {f}");
        assert!((balanced(&t, 0.0, 10.0) - 5.0).abs() < 1e-6);
        let (c, f) = volumes(&t, 0.0);
        assert!((c - 500.0).abs() < 1e-9 && f.abs() < 1e-9, "all cut: {c} {f}");
    }

    #[test]
    fn an_l_shaped_boundary_limits_the_volume() {
        // 20 × 20 ground 1 m above the design; the boundary is an L of 300 square units.
        let t = square(|_, _| 1.0, 20.0);
        let l =
            [Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 10.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 20.0), Vec2::new(0.0, 20.0)];
        let pieces = limit_to(&t, Some(&l)).unwrap();
        let (c, f) = volumes(&pieces, 0.0);
        assert!((c - 300.0).abs() < 1e-9 && f == 0.0, "{c} {f}");
        // Clockwise gives the same answer.
        let rev: Vec<Vec2> = l.iter().rev().copied().collect();
        let (c2, _) = volumes(&limit_to(&t, Some(&rev)).unwrap(), 0.0);
        assert!((c2 - 300.0).abs() < 1e-9);
    }

    #[test]
    fn a_crossing_boundary_is_refused() {
        let bow = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0)];
        assert!(ear_clip(&bow).is_none());
        assert!(ear_clip(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(2.0, 2.0)]).is_none());
    }
}
