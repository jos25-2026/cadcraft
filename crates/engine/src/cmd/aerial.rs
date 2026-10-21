//! AERIAL: bring an aerial photo from an open imagery service into the drawing, in the drawing's
//! coordinate system. The area is given as an extent in the chosen system (WGS 84 UTM, any zone, or
//! Web Mercator) or as a longitude/latitude box; the command downloads the Web Mercator tiles that
//! cover it, re-projects them onto a north-up grid of the chosen system and saves a PNG with a
//! world file (`.pgw`) and a `.prj`, then places it like IMAGEATTACH, where the coordinates say.
//!
//! Only services whose terms allow this use are offered (see [`SOURCES`]); each carries the
//! attribution its licence asks for, which is drawn under the picture.
//!
//! The download goes through a [`TileFetcher`] kept in the session (the tests give one that
//! serves synthetic tiles); without one the command uses a plain HTTPS client on native builds
//! and says it is not available in the browser build.

use std::f64::consts::PI;
use std::sync::Arc;

use cadcraft_color::Color;
use cadcraft_doc::{Common, EntityKind, HAlign, Image, VAlign};
use cadcraft_geom::{Crs, Vec2, Vec3};
use serde_json::{Value, json};

use super::draw::make_text;
use super::mapgrid::default_extent;
use super::*;
use crate::{Result, Session};

/// Tiles fetched at most for one picture, and the output limits.
const MAX_TILES: i64 = 400;
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 80_000_000;
const MAX_TILE_BYTES: u64 = 3_000_000;
const TILE: u32 = 256;
/// The header variable that remembers the drawing's coordinate system (a user string).
pub const CRS_VAR: &str = "USERS5";
/// Metres per pixel of a 256 pixel Web Mercator tile at zoom 0 on the equator.
const ZOOM0_RES: f64 = 156_543.033_928_041;

/// An open imagery service.
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub id: &'static str,
    pub name: &'static str,
    /// The tile address with `{z}`, `{x}` and `{y}` (Web Mercator, 256 pixel tiles).
    pub url: &'static str,
    pub max_zoom: u8,
    /// Where it has coverage: west, south, east, north (degrees).
    pub bounds: [f64; 4],
    pub attribution: &'static str,
    pub licence: &'static str,
    pub coverage: &'static str,
    pub detail: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source {
        id: "eox-s2",
        name: "Sentinel-2 cloudless (EOX)",
        url: "https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless_3857/default/g/{z}/{y}/{x}.jpg",
        max_zoom: 13,
        bounds: [-180.0, -85.0, 180.0, 85.0],
        attribution: "Sentinel-2 cloudless - https://s2maps.eu by EOX IT Services GmbH (Contains modified Copernicus Sentinel data 2016)",
        licence: "CC BY 4.0",
        coverage: "worldwide",
        detail: "about 10 m per pixel",
    },
    Source {
        id: "usgs-imagery",
        name: "USGS National Map imagery",
        url: "https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/{z}/{y}/{x}",
        max_zoom: 19,
        bounds: [-180.0, 15.0, -60.0, 72.0],
        attribution: "USDA, USGS The National Map: Orthoimagery",
        licence: "public domain (U.S. Government work)",
        coverage: "United States, including Alaska and Hawaii",
        detail: "0.3 to 1 m per pixel in most places",
    },
];

/// Downloads one tile.
pub trait TileFetcher: Send + Sync {
    fn get(&self, url: &str) -> std::result::Result<Vec<u8>, String>;
}

#[cfg(not(target_arch = "wasm32"))]
struct HttpFetcher {
    agent: ureq::Agent,
}

#[cfg(not(target_arch = "wasm32"))]
impl HttpFetcher {
    fn new() -> HttpFetcher {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(25)))
            .user_agent(concat!("CADCraft/", env!("CARGO_PKG_VERSION"), " (aerial image tool; https://github.com/storytold/cadcraft)"))
            .build();
        HttpFetcher { agent: ureq::Agent::new_with_config(config) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl TileFetcher for HttpFetcher {
    fn get(&self, url: &str) -> std::result::Result<Vec<u8>, String> {
        let mut resp = self.agent.get(url).call().map_err(|e| e.to_string())?;
        resp.body_mut().with_config().limit(MAX_TILE_BYTES).read_to_vec().map_err(|e| e.to_string())
    }
}

fn default_fetcher() -> Option<Arc<dyn TileFetcher>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Arc::new(HttpFetcher::new()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("aerial", "Aerial Image...", run_aerial).alias(&["aerialimage", "onlineimage", "satellite"]).params(
        "{source?: \"eox-s2\" (worldwide, 10 m) | \"usgs-imagery\" (United States, under 1 m), crs?: \"UTM 47N\" | \"EPSG:32647\" | \"EPSG:3857\" \
                 (default: the drawing's, else the UTM zone of the area), extent?: [minX, minY, maxX, maxY] in that system (default: the drawing), \
                 lonLat?: [west, south, east, north] degrees (instead of extent), resolution?: metres per pixel (default: the best the source has, \
                 within 4096 pixels), path?: where to save the PNG (default: beside the drawing, else the temporary folder), \
                 attribution?: true (draw the credit under the picture), layer?: \"AERIAL\"} \
                 → {path, worldFile, crs, epsg, source, zoom, size, resolution, extent, tiles, missingTiles, attribution, licence, handle}",
    )]
}

// ---- Web Mercator pixels ----

/// Longitude and latitude to the Web Mercator square in `0..1` (x east, y south).
fn mercator_unit(lon: f64, lat: f64) -> Option<(f64, f64)> {
    if !(lon.is_finite() && lat.is_finite() && lat.abs() <= cadcraft_geom::crs::MERCATOR_MAX_LAT) {
        return None;
    }
    let s = lat.to_radians().sin();
    let y = 0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * PI);
    Some(((lon + 180.0) / 360.0, y))
}

fn ground_res(zoom: u32, lat: f64) -> f64 {
    ZOOM0_RES * lat.to_radians().cos().abs().max(1e-6) / f64::from(1u32 << zoom.min(30))
}

fn tile_url(src: &Source, z: u32, x: i64, y: i64) -> String {
    src.url.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

fn lookup(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.id.eq_ignore_ascii_case(id.trim()))
}

fn pair(p: &Value, key: &str) -> Result<Option<[f64; 4]>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let a: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    match a[..] {
        [a, b, c, d] if [a, b, c, d].iter().all(|x| x.is_finite() && x.abs() <= 1.0e9) && a < c && b < d => Ok(Some([a, b, c, d])),
        _ => Err(bad("aerial", format!("`{key}` must be [min x, min y, max x, max y] with min below max"))),
    }
}

/// The part of a path that is the folder, or the temporary folder for a drawing never saved.
fn output_folder(s: &Session) -> std::path::PathBuf {
    let beside =
        s.docs.get(s.active).and_then(|d| d.path.as_deref()).and_then(|p| std::path::Path::new(p).parent().map(std::path::Path::to_path_buf));
    match beside {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => std::env::temp_dir().join("cadcraft-aerial"),
    }
}

fn run_aerial(s: &mut Session, p: &Value) -> Result<Value> {
    let src = match str_param(p, "source") {
        None => &SOURCES[0],
        Some(id) => lookup(id).ok_or_else(|| {
            bad("aerial", format!("no source \"{id}\": use {}", SOURCES.iter().map(|x| format!("\"{}\"", x.id)).collect::<Vec<_>>().join(" or ")))
        })?,
    };
    // The coordinate system: asked for, else the drawing's, else the zone of the area.
    let asked = match str_param(p, "crs") {
        Some(t) => {
            Some(Crs::parse(t).ok_or_else(|| bad("aerial", format!("unknown coordinate system \"{t}\": use UTM 47N, EPSG:32647 or EPSG:3857")))?)
        }
        None => s.doc().ok().and_then(|d| Crs::parse(&d.header.str(CRS_VAR, ""))),
    };
    let lonlat = pair(p, "lonLat")?;
    let extent_param = pair(p, "extent")?;
    let crs = match (asked, lonlat) {
        (Some(c), _) => c,
        (None, Some([w, so, e, n])) => Crs::utm_for((w + e) / 2.0, (so + n) / 2.0).ok_or_else(|| bad("aerial", "`lonLat` is out of range"))?,
        (None, None) => return Err(bad("aerial", "`crs` (the drawing's coordinate system, e.g. \"UTM 47N\") is required")),
    };
    if crs.is_geographic() {
        return Err(bad("aerial", "a drawing in degrees cannot hold a photo to scale: use a UTM zone or EPSG:3857"));
    }
    // The area in the drawing's system.
    let [min_x, min_y, max_x, max_y] = match (lonlat, extent_param) {
        (Some(b), _) => {
            let [w, so, e, n] = b;
            if !(w >= -180.0 && e <= 180.0 && so >= -90.0 && n <= 90.0) {
                return Err(bad("aerial", "`lonLat` is [west, south, east, north] in degrees"));
            }
            let (mut lo, mut hi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
            for i in 0..=8 {
                for j in 0..=8 {
                    let (lon, lat) = (w + (e - w) * f64::from(i) / 8.0, so + (n - so) * f64::from(j) / 8.0);
                    if let Some((x, y)) = crs.from_lonlat(lon, lat) {
                        lo = Vec2::new(lo.x.min(x), lo.y.min(y));
                        hi = Vec2::new(hi.x.max(x), hi.y.max(y));
                    }
                }
            }
            if !(lo.x.is_finite() && hi.x.is_finite()) {
                return Err(bad("aerial", "the area cannot be expressed in this coordinate system (too far from its zone)"));
            }
            [lo.x, lo.y, hi.x, hi.y]
        }
        (None, Some(b)) => b,
        (None, None) => {
            let b = default_extent(s).map_err(|_| bad("aerial", "give `extent` or `lonLat`, or draw something to take the area from"))?;
            let pad = ((b.max.x - b.min.x).max(b.max.y - b.min.y) * 0.05).max(1.0);
            [b.min.x - pad, b.min.y - pad, b.max.x + pad, b.max.y + pad]
        }
    };
    let (width, height) = (max_x - min_x, max_y - min_y);
    if !(width > 0.0 && height > 0.0) || width.max(height) > 2.0e6 {
        return Err(bad("aerial", "the area must be between a few metres and 2000 km across"));
    }
    // Longitude and latitude of the area's corners and centre decide coverage and zoom.
    let corners: Vec<(f64, f64)> = [(min_x, min_y), (max_x, min_y), (max_x, max_y), (min_x, max_y), ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0)]
        .iter()
        .filter_map(|&(x, y)| crs.to_lonlat(x, y))
        .collect();
    if corners.len() != 5 {
        return Err(bad("aerial", "the area is outside the range of the coordinate system: check the zone and the numbers"));
    }
    let clat = corners.get(4).map_or(0.0, |c| c.1);
    let (lo_lon, hi_lon) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, c| (a.0.min(c.0), a.1.max(c.0)));
    let (lo_lat, hi_lat) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, c| (a.0.min(c.1), a.1.max(c.1)));
    let [bw, bs, be, bn] = src.bounds;
    if hi_lon < bw || lo_lon > be || hi_lat < bs || lo_lat > bn {
        return Err(bad("aerial", format!("the area is outside the coverage of {}: {}", src.name, src.coverage)));
    }
    // Resolution and zoom.
    let native = ground_res(u32::from(src.max_zoom), clat);
    let wanted = match p.get("resolution").filter(|v| !v.is_null()) {
        None => native.max(width.max(height) / 4096.0),
        Some(v) => {
            v.as_f64().filter(|r| r.is_finite() && *r > 0.0).ok_or_else(|| bad("aerial", "`resolution` must be positive (metres per pixel)"))?
        }
    };
    let (w_px, h_px) = ((width / wanted).ceil(), (height / wanted).ceil());
    if !(w_px >= 1.0 && h_px >= 1.0 && w_px <= f64::from(MAX_SIDE) && h_px <= f64::from(MAX_SIDE) && w_px * h_px <= MAX_PIXELS as f64) {
        return Err(bad("aerial", "the picture would be too large: give a coarser `resolution` or a smaller area"));
    }
    let (w_px, h_px) = (w_px as u32, h_px as u32);
    let (dx, dy) = (width / f64::from(w_px), height / f64::from(h_px));
    let mut zoom = (ZOOM0_RES * clat.to_radians().cos().abs().max(1e-6) / wanted).log2().ceil().clamp(0.0, f64::from(src.max_zoom)) as u32;

    // Where each output pixel is in the source: exact on a coarse grid, interpolated between.
    const STEP: u32 = 16;
    let (gx, gy) = (w_px.div_ceil(STEP) + 1, h_px.div_ceil(STEP) + 1);
    let mut grid: Vec<Option<(f64, f64)>> = Vec::with_capacity((gx * gy) as usize);
    for b in 0..gy {
        for a in 0..gx {
            let (x, y) = (min_x + f64::from(a * STEP) * dx, max_y - f64::from(b * STEP) * dy);
            grid.push(crs.to_lonlat(x, y).and_then(|(lon, lat)| mercator_unit(lon, lat)));
        }
    }
    let (mut lo, mut hi) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
    for (u, v) in grid.iter().flatten() {
        lo = (lo.0.min(*u), lo.1.min(*v));
        hi = (hi.0.max(*u), hi.1.max(*v));
    }
    if !(lo.0.is_finite() && hi.0.is_finite()) {
        return Err(bad("aerial", "the area is outside the range of the web map (beyond 85 degrees of latitude)"));
    }
    let mut note = None;
    let range = |zoom: u32| {
        let n = f64::from(1u32 << zoom);
        let last = (1i64 << zoom) - 1;
        let t = |u: f64| ((u * n).floor() as i64).clamp(0, last);
        (
            t(lo.0 - 0.5 / (n * f64::from(TILE))),
            t(lo.1 - 0.5 / (n * f64::from(TILE))),
            t(hi.0 + 1.0 / (n * f64::from(TILE))),
            t(hi.1 + 1.0 / (n * f64::from(TILE))),
        )
    };
    let mut r = range(zoom);
    while (r.2 - r.0 + 1) * (r.3 - r.1 + 1) > MAX_TILES && zoom > 0 {
        zoom -= 1;
        r = range(zoom);
        note = Some(format!("zoom reduced to {zoom} to keep the download under {MAX_TILES} tiles: the picture is coarser than asked"));
    }
    let (tx0, ty0, tx1, ty1) = r;
    let (nx, ny) = (tx1 - tx0 + 1, ty1 - ty0 + 1);
    let n_tiles = nx * ny;
    if n_tiles > MAX_TILES {
        return Err(bad("aerial", "the area needs too many tiles: choose a smaller area"));
    }

    // Download.
    let fetcher = match &s.tile_fetcher {
        Some(f) => Arc::clone(f),
        None => default_fetcher().ok_or_else(|| bad("aerial", "downloading aerial photos is not available in the web version yet"))?,
    };
    let wanted_tiles: Vec<(i64, i64)> = (ty0..=ty1).flat_map(|y| (tx0..=tx1).map(move |x| (x, y))).collect();
    let results = fetch_all(&*fetcher, src, zoom, &wanted_tiles);
    let mut tiles: Vec<Option<image::RgbImage>> = Vec::with_capacity(wanted_tiles.len());
    let mut first_error = None;
    for r in results {
        match r.and_then(|bytes| decode(&bytes)) {
            Ok(img) => tiles.push(Some(img)),
            Err(e) => {
                first_error.get_or_insert(e);
                tiles.push(None);
            }
        }
    }
    let got = tiles.iter().filter(|t| t.is_some()).count();
    if got == 0 {
        return Err(bad("aerial", format!("no tile could be downloaded from {}: {}", src.name, first_error.unwrap_or_else(|| "no answer".into()))));
    }
    let missing = tiles.len() - got;

    // Re-project into a north-up picture of the drawing's system.
    let world = f64::from(TILE) * f64::from(1u32 << zoom);
    let mut out = image::RgbaImage::new(w_px, h_px);
    let at = |ix: i64, iy: i64| -> Option<[f32; 3]> {
        let (tcol, trow) = (ix.div_euclid(i64::from(TILE)) - tx0, iy.div_euclid(i64::from(TILE)) - ty0);
        if tcol < 0 || trow < 0 || tcol >= nx || trow >= ny {
            return None;
        }
        let tile = tiles.get(usize::try_from(trow * nx + tcol).ok()?)?.as_ref()?;
        let px = tile.get_pixel_checked(u32::try_from(ix.rem_euclid(i64::from(TILE))).ok()?, u32::try_from(iy.rem_euclid(i64::from(TILE))).ok()?)?;
        Some([f32::from(px.0[0]), f32::from(px.0[1]), f32::from(px.0[2])])
    };
    for j in 0..h_px {
        let (b, fy) = (j / STEP, (f64::from(j) + 0.5) / f64::from(STEP) - f64::from(j / STEP));
        for i in 0..w_px {
            let (a, fx) = (i / STEP, (f64::from(i) + 0.5) / f64::from(STEP) - f64::from(i / STEP));
            let node = |aa: u32, bb: u32| grid.get((bb * gx + aa) as usize).copied().flatten();
            let (Some(n00), Some(n10), Some(n01), Some(n11)) = (node(a, b), node(a + 1, b), node(a, b + 1), node(a + 1, b + 1)) else { continue };
            let lerp = |p: f64, q: f64, t: f64| p + (q - p) * t;
            let sx = lerp(lerp(n00.0, n10.0, fx), lerp(n01.0, n11.0, fx), fy) * world;
            let sy = lerp(lerp(n00.1, n10.1, fx), lerp(n01.1, n11.1, fx), fy) * world;
            // Bilinear between the four source pixels around (sx, sy); pixel centres sit at +0.5.
            let (px, py) = (sx - 0.5, sy - 0.5);
            let (ix, iy) = (px.floor(), py.floor());
            if !(ix.is_finite() && iy.is_finite() && ix.abs() < 1.0e9 && iy.abs() < 1.0e9) {
                continue;
            }
            let (ix, iy, tx, ty) = (ix as i64, iy as i64, (px - ix) as f32, (py - iy) as f32);
            let nearest = at(sx.floor() as i64, sy.floor() as i64);
            let Some(base) = nearest else { continue };
            let s00 = at(ix, iy).unwrap_or(base);
            let s10 = at(ix + 1, iy).unwrap_or(base);
            let s01 = at(ix, iy + 1).unwrap_or(base);
            let s11 = at(ix + 1, iy + 1).unwrap_or(base);
            let mut rgb = [0u8; 3];
            for (k, slot) in rgb.iter_mut().enumerate() {
                let g = |c: [f32; 3]| c.get(k).copied().unwrap_or(0.0);
                let top = g(s00) + (g(s10) - g(s00)) * tx;
                let bot = g(s01) + (g(s11) - g(s01)) * tx;
                *slot = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
            out.put_pixel(i, j, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        }
    }

    // Save the picture, its world file and projection, then place it.
    let folder = match str_param(p, "path") {
        Some(t) => std::path::Path::new(t)
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
        None => output_folder(s),
    };
    std::fs::create_dir_all(&folder).map_err(|e| bad("aerial", format!("{}: {e}", folder.display())))?;
    let file = match str_param(p, "path") {
        Some(t) => std::path::Path::new(t).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "aerial.png".into()),
        None => format!("aerial_{}_{}_{:.0}_{:.0}_{:.2}.png", src.id.replace('-', ""), crs.epsg(), min_x, min_y, dx),
    };
    let file = if file.to_ascii_lowercase().ends_with(".png") { file } else { format!("{file}.png") };
    let png = folder.join(&file);
    out.save_with_format(&png, image::ImageFormat::Png).map_err(|e| bad("aerial", format!("{}: {e}", png.display())))?;
    let stem = png.with_extension("");
    let world_text = format!("{}\n0\n0\n{}\n{}\n{}\n", dx, -dy, min_x + dx / 2.0, max_y - dy / 2.0);
    let pgw = stem.with_extension("pgw");
    std::fs::write(&pgw, world_text).map_err(|e| bad("aerial", format!("{}: {e}", pgw.display())))?;
    std::fs::write(stem.with_extension("prj"), crs.wkt()).map_err(|e| bad("aerial", format!("{}: {e}", stem.display())))?;
    let path_text = png.to_string_lossy().into_owned();
    let stored =
        cadcraft_doc::plain_file_path(&std::fs::canonicalize(&png).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| path_text.clone()));

    let layer = str_param(p, "layer").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("AERIAL").to_string();
    let img = Image {
        insert: Vec3::new(min_x, min_y, 0.0),
        u: Vec3::new(dx, 0.0, 0.0),
        v: Vec3::new(0.0, dy, 0.0),
        size: Vec2::new(f64::from(w_px), f64::from(h_px)),
        path: stored.clone(),
        name: format!("{} {}", src.name, crs.name()),
        ..Image::default()
    };
    let draw_credit = bool_or(p, "attribution", true);
    let th = (width.max(height) / 120.0).max(dx * 6.0);
    let credit = format!("{} - {} - {}", src.attribution, src.licence, crs.name());
    let space = s.space();
    let base = s.current_common();
    let handle = {
        let d = s.doc_mut()?;
        let colour = if d.ensure_layer(&layer) { d.layers.iter_mut().find(|l| l.name == layer).map(|l| l.color = Color::Index(8)) } else { None };
        let _ = colour;
        let handle = d.add(&space, Common { layer: layer.clone(), color: Color::ByLayer, ..base.clone() }, EntityKind::Image(img))?;
        d.header.set_str(CRS_VAR, &format!("EPSG:{}", crs.epsg()));
        handle
    };
    if draw_credit {
        let t = make_text(s, Vec2::new(min_x, min_y - th * 1.5), &credit, th, 0.0, (HAlign::Left, VAlign::Baseline));
        let d = s.doc_mut()?;
        d.add(&space, Common { layer: format!("{layer}-CREDIT"), color: Color::ByLayer, ..base }, t)?;
    }
    let warning = cadcraft_render::image_layer::problem(&stored);
    let mut msg = format!(
        "{} aerial photo {w_px} x {h_px} px ({dx:.2} m) placed in {}: E {min_x:.1} to {max_x:.1}, N {min_y:.1} to {max_y:.1}",
        src.name,
        crs.name()
    );
    if missing > 0 {
        msg.push_str(&format!("; {missing} of {} tiles could not be downloaded (left empty)", wanted_tiles.len()));
    }
    if let Some(n) = &note {
        msg.push_str(&format!("; {n}"));
    }
    s.echo(msg.clone());
    Ok(json!({
        "path": path_text, "worldFile": pgw.to_string_lossy(), "crs": crs.name(), "epsg": crs.epsg(), "source": src.id,
        "zoom": zoom, "size": [w_px, h_px], "resolution": [dx, dy], "extent": [min_x, min_y, max_x, max_y],
        "tiles": wanted_tiles.len(), "missingTiles": missing, "attribution": src.attribution, "licence": src.licence,
        "handle": handle.hex(), "warning": warning, "note": note, "message": msg,
    }))
}

/// Downloads the tiles with a few threads, in order.
fn fetch_all(f: &dyn TileFetcher, src: &Source, zoom: u32, wanted: &[(i64, i64)]) -> Vec<std::result::Result<Vec<u8>, String>> {
    let last = (1i64 << zoom) - 1;
    let threads = 6usize.min(wanted.len().max(1));
    let mut results: Vec<std::result::Result<Vec<u8>, String>> = vec![Err("not fetched".into()); wanted.len()];
    let chunks: Vec<Vec<(usize, (i64, i64))>> =
        (0..threads).map(|t| wanted.iter().copied().enumerate().filter(|(i, _)| i % threads == t).collect()).collect();
    let done: Vec<Vec<(usize, std::result::Result<Vec<u8>, String>)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                sc.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(i, (x, y))| {
                            let r = if x < 0 || y < 0 || x > last || y > last {
                                Err("outside the map".to_string())
                            } else {
                                f.get(&tile_url(src, zoom, x, y))
                            };
                            (i, r)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    for (i, r) in done.into_iter().flatten() {
        if let Some(slot) = results.get_mut(i) {
            *slot = r;
        }
    }
    results
}

fn decode(bytes: &[u8]) -> std::result::Result<image::RgbImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgb8();
    if img.width() != TILE || img.height() != TILE {
        return Err(format!("a {} x {} tile, not {TILE} x {TILE}", img.width(), img.height()));
    }
    Ok(img)
}
