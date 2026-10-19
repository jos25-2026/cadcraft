//! TREES: plants a tree symbol at every survey point on the chosen layers. Import the points with
//! `layerByCode` and each description code (TREE, PALM, ...) has its own layer to choose from.
//! The symbols are plain circles, arcs, lines and polylines on their own layer.

use cadcraft_doc::{Arc, Circle, Common, EntityKind, Line, LwPolyline};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most trees one command plants.
const MAX_TREES: usize = 50_000;
/// Most layers one command reads.
const MAX_LAYERS: usize = 1000;
/// Largest coordinate or size accepted.
const MAX_COORD: f64 = 1.0e12;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("trees", "Plant Trees...", run_trees).alias(&["planttrees", "treesymbols"]).menu(&["Insert", "Trees..."]).params(
        "{layers: [names of the layers holding the POINTs, e.g. \"SURVEY-PTS-TREE\"], size?: crown diameter (default 5), \
         style?: \"round\" | \"cloud\" | \"conifer\" | \"palm\" (default round), layer?: \"TREES\"} → {planted, layers}",
    )]
}

fn v3(p: Vec2, z: f64) -> Vec3 {
    Vec3::new(p.x, p.y, z)
}

fn poly(pts: Vec<Vec2>) -> EntityKind {
    EntityKind::LwPolyline(LwPolyline {
        vertices: pts.into_iter().map(PolyVertex::new).collect(),
        closed: true,
        const_width: 0.0,
        elevation: 0.0,
        plinegen: false,
    })
}

/// The symbol for one tree of crown radius `r` centred on `c`.
fn symbol(style: &str, c: Vec2, z: f64, r: f64) -> Vec<EntityKind> {
    let at = |x: f64, y: f64| c + Vec2::new(x * r, y * r);
    let line = |a: Vec2, b: Vec2| EntityKind::Line(Line { a: v3(a, z), b: v3(b, z) });
    let trunk = EntityKind::Circle(Circle { center: v3(c, z), radius: r * 0.08 });
    match style {
        "cloud" => {
            // Eight bumps round the crown.
            let mut v: Vec<EntityKind> = (0..8)
                .map(|i| {
                    let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
                    let bump = r * 0.3;
                    let centre = c + Vec2::new(a.cos(), a.sin()) * (r - bump);
                    EntityKind::Arc(Arc {
                        center: v3(centre, z),
                        radius: bump * 1.05,
                        start: a - std::f64::consts::FRAC_PI_2 - 0.2,
                        end: a + std::f64::consts::FRAC_PI_2 + 0.2,
                    })
                })
                .collect();
            v.push(trunk);
            v
        }
        "conifer" => vec![poly(vec![at(0.0, 1.0), at(-0.87, -0.5), at(0.87, -0.5)]), line(at(0.0, 1.0), at(0.0, -0.5)), trunk],
        "palm" => {
            // Six pointed fronds.
            let mut v: Vec<EntityKind> = (0..6)
                .map(|i| {
                    let a = f64::from(i) * std::f64::consts::FRAC_PI_3;
                    let (u, w) = (Vec2::new(a.cos(), a.sin()), Vec2::new(-a.sin(), a.cos()));
                    poly(vec![c, c + u * (r * 0.5) + w * (r * 0.18), c + u * r, c + u * (r * 0.5) - w * (r * 0.18)])
                })
                .collect();
            v.push(trunk);
            v
        }
        _ => vec![
            EntityKind::Circle(Circle { center: v3(c, z), radius: r }),
            line(at(-0.15, 0.0), at(0.15, 0.0)),
            line(at(0.0, -0.15), at(0.0, 0.15)),
            trunk,
        ],
    }
}

fn run_trees(s: &mut Session, p: &Value) -> Result<Value> {
    let names: Vec<String> = match p.get("layers") {
        Some(Value::Array(a)) if a.len() <= MAX_LAYERS => a.iter().filter_map(|v| v.as_str()).map(|t| t.trim().to_lowercase()).collect(),
        Some(Value::Array(_)) => return Err(bad("trees", format!("more than {MAX_LAYERS} layers"))),
        _ => Vec::new(),
    };
    if names.is_empty() {
        return Err(bad("trees", "`layers` (the layers holding the points, e.g. SURVEY-PTS-TREE) is required"));
    }
    let size = size_param("trees", p, "size", false)?.unwrap_or(5.0);
    if size > MAX_COORD {
        return Err(bad("trees", "`size` is too large"));
    }
    let style = str_param(p, "style").unwrap_or("round").to_ascii_lowercase();
    if !["round", "cloud", "conifer", "palm"].contains(&style.as_str()) {
        return Err(bad("trees", "`style` must be \"round\", \"cloud\", \"conifer\" or \"palm\""));
    }
    let layer = super::terrain::layer_name("trees", str_param(p, "layer").unwrap_or("TREES"))?;

    let mut spots: Vec<Vec3> = Vec::new();
    let mut found = std::collections::BTreeSet::new();
    {
        let d = s.doc()?;
        for e in d.model.iter() {
            let lname = e.common.layer.to_lowercase();
            if let EntityKind::Point(pt) = &e.kind
                && names.contains(&lname)
                && pt.p.x.is_finite()
                && pt.p.y.is_finite()
                && pt.p.z.is_finite()
                && pt.p.x.abs() <= MAX_COORD
                && pt.p.y.abs() <= MAX_COORD
            {
                found.insert(e.common.layer.clone());
                if spots.len() >= MAX_TREES {
                    return Err(bad("trees", format!("more than {MAX_TREES} trees: choose fewer layers")));
                }
                spots.push(pt.p);
            }
        }
    }
    if spots.is_empty() {
        return Err(bad("trees", "no points on those layers (import survey points first, with layerByCode to get one layer per code)"));
    }

    let space = s.space();
    let base = s.current_common();
    {
        let d = s.doc_mut()?;
        if d.ensure_layer(&layer)
            && let Some(l) = d.layers.iter_mut().find(|l| l.name == layer)
        {
            l.color = cadcraft_color::Color::Index(3);
        }
    }
    let com = Common { layer: layer.clone(), color: cadcraft_color::Color::ByLayer, ..base };
    let r = size / 2.0;
    let d = s.doc_mut()?;
    for pt in &spots {
        for k in symbol(&style, Vec2::new(pt.x, pt.y), pt.z, r) {
            d.add(&space, com.clone(), k)?;
        }
    }
    let msg = format!("{} trees planted on layer {layer}", spots.len());
    s.echo(msg.clone());
    Ok(json!({ "planted": spots.len(), "layers": found, "layer": layer, "size": size, "style": style, "message": msg }))
}
