//! IMAGEATTACH: place a raster image (an aerial photo, a scanned plan) in the drawing. A world
//! file beside the image (`.pgw` for PNG, `.jgw` for JPEG, `.tfw` for TIFF, or `.wld`) gives its
//! position, scale and rotation in drawing coordinates, so the photo lands where it was taken.
//! Without one the image goes at `insert` (default 0,0) at `scale` drawing units per pixel.

use cadcraft_doc::{EntityKind, Image};
use cadcraft_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Largest image side accepted, in pixels (a corrupt header can claim anything).
const MAX_SIDE: u32 = 1 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("imageattach", "Attach Raster Image", run_attach)
            .alias(&["iat"])
            .params(
                "{path, insert?: [x,y], scale?: drawing units per pixel (default 1), rotation?: degrees, \
                 worldFile?: path | false (default: the file beside the image)}. A world file sets position, \
                 scale and rotation unless `insert` or `scale` is given.",
            )
            .interactive(|_| Ok(Box::new(AttachM))),
    ]
}

/// Asks for the image file, then places it as the JSON form does (world file beside it, if any).
struct AttachM;

impl Interactive for AttachM {
    fn name(&self) -> &'static str {
        "IMAGEATTACH"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter the image file path (PNG, JPEG or TIFF; a world file beside it sets the position)", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Input::Text(t) = i else { return Ok(Step::Done) };
        let t = t.trim().trim_matches('"');
        if t.is_empty() {
            return Ok(Step::Done);
        }
        run_attach(s, &json!({ "path": t }))?;
        Ok(Step::Done)
    }
}

/// The six numbers of a world file: x pixel size A, rotation D, rotation B, y pixel size E
/// (negative for a north-up image), and the centre of the top-left pixel (C, F).
#[derive(Clone, Copy, Debug, PartialEq)]
struct World {
    a: f64,
    d: f64,
    b: f64,
    e: f64,
    c: f64,
    f: f64,
}

fn parse_world(text: &str) -> std::result::Result<World, String> {
    let nums: Vec<f64> = text
        .split_whitespace()
        .take(6)
        .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()
        .ok_or("a world file holds six numbers, one per line")?;
    let [a, d, b, e, c, f] = nums[..] else {
        return Err("a world file holds six numbers, one per line".into());
    };
    // A zero determinant would flatten the image to a line.
    if (a * e - b * d).abs() < 1e-30 {
        return Err("the world file's pixel size is zero".into());
    }
    Ok(World { a, d, b, e, c, f })
}

/// Placement of an image `h` pixels tall from a world file: the lower-left corner and the
/// per-pixel vectors (`u` along the row, `v` up the image).
fn place_world(wf: World, h: f64) -> (Vec2, Vec2, Vec2) {
    let u = Vec2::new(wf.a, wf.d);
    let v = Vec2::new(-wf.b, -wf.e);
    // (C, F) is the centre of the top-left pixel: step half a pixel out to the corner, then down
    // the image by its height to the lower-left corner.
    let upper_left = Vec2::new(wf.c - 0.5 * (wf.a + wf.b), wf.f - 0.5 * (wf.d + wf.e));
    let lower_left = Vec2::new(upper_left.x + h * wf.b, upper_left.y + h * wf.e);
    (lower_left, u, v)
}

/// World file name candidates beside `image`, most specific first.
fn world_candidates(image: &str) -> Vec<String> {
    let (stem, ext) = match image.rsplit_once('.') {
        Some((s, e)) if !e.contains(['/', '\\']) => (s, e.to_ascii_lowercase()),
        _ => (image, String::new()),
    };
    let short: &[&str] = match ext.as_str() {
        "png" => &["pgw", "pngw"],
        "jpg" | "jpeg" => &["jgw", "jpgw", "jpegw"],
        "tif" | "tiff" => &["tfw", "tifw", "tiffw"],
        "bmp" => &["bpw", "bmpw"],
        "gif" => &["gfw", "gifw"],
        _ => &[],
    };
    let mut out = Vec::new();
    for e in short.iter().copied().chain(["wld"]) {
        out.push(format!("{stem}.{e}"));
        out.push(format!("{stem}.{}", e.to_ascii_uppercase()));
    }
    out
}

fn point2(p: &Value, key: &str) -> Result<Option<Vec2>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() >= 2).ok_or_else(|| bad("imageattach", format!("`{key}` must be [x, y]")))?;
            match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Some(Vec2::new(x, y))),
                _ => Err(bad("imageattach", format!("`{key}` must be [x, y] numbers"))),
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn image_size(path: &str) -> Result<(u32, u32)> {
    let reader = image::ImageReader::open(path).map_err(|e| bad("imageattach", format!("{path}: {e}")))?;
    let reader = reader.with_guessed_format().map_err(|e| bad("imageattach", format!("{path}: {e}")))?;
    let (w, h) = reader.into_dimensions().map_err(|e| bad("imageattach", format!("{path}: not a readable image ({e})")))?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(bad("imageattach", format!("{path}: image size {w} × {h} is not supported")));
    }
    Ok((w, h))
}

#[cfg(target_arch = "wasm32")]
fn image_size(_path: &str) -> Result<(u32, u32)> {
    Err(bad("imageattach", "paths are not available on the web"))
}

fn read_world(p: &Value, path: &str) -> Result<Option<(World, String)>> {
    let explicit = match p.get("worldFile") {
        Some(Value::Bool(false)) => return Ok(None),
        Some(Value::String(s)) => Some(s.clone()),
        None | Some(Value::Null) | Some(Value::Bool(true)) => None,
        Some(_) => return Err(bad("imageattach", "`worldFile` must be a path or false")),
    };
    let candidates = explicit.clone().map(|e| vec![e]).unwrap_or_else(|| world_candidates(path));
    for c in candidates {
        match std::fs::read_to_string(&c) {
            Ok(t) => return parse_world(&t).map(|w| Some((w, c.clone()))).map_err(|m| bad("imageattach", format!("{c}: {m}"))),
            Err(e) if explicit.is_some() => return Err(bad("imageattach", format!("{c}: {e}"))),
            Err(_) => {}
        }
    }
    Ok(None)
}

fn run_attach(s: &mut Session, p: &Value) -> Result<Value> {
    let Some(path) = str_param(p, "path") else {
        return Err(bad("imageattach", "`path` (an image file) is required"));
    };
    let (w, h) = image_size(path)?;
    let (wf, hf) = (f64::from(w), f64::from(h));
    let insert = point2(p, "insert")?;
    let scale = size_param("imageattach", p, "scale", false)?;
    let rotation = f64_or(p, "rotation", 0.0).to_radians();
    // Absolute, so the drawing finds the image wherever it is opened from.
    let stored =
        cadcraft_doc::plain_file_path(&std::fs::canonicalize(path).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string()));

    let world = if insert.is_none() && scale.is_none() { read_world(p, path)? } else { None };
    let (origin, u, v) = match &world {
        Some((wd, _)) => place_world(*wd, hf),
        None => {
            let k = scale.unwrap_or(1.0);
            let (sn, cs) = rotation.sin_cos();
            (insert.unwrap_or(Vec2::ZERO), Vec2::new(k * cs, k * sn), Vec2::new(-k * sn, k * cs))
        }
    };
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let img = Image {
        insert: Vec3::new(origin.x, origin.y, 0.0),
        u: Vec3::new(u.x, u.y, 0.0),
        v: Vec3::new(v.x, v.y, 0.0),
        size: Vec2::new(wf, hf),
        path: stored.clone(),
        name,
        ..Image::default()
    };
    let corners = [origin, origin + u * wf, origin + u * wf + v * hf, origin + v * hf];
    let handle = s.add_entity(EntityKind::Image(img))?;
    let mut msg = match &world {
        Some((_, f)) => format!("image {w} × {h} placed from {f}"),
        None => format!("image {w} × {h} placed at {:.3},{:.3}", origin.x, origin.y),
    };
    // Where it landed, so a picture far from the rest of the drawing can be found (ZOOM Extents).
    let (lo, hi) = corners.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(l, h), c| {
        (Vec2::new(l.x.min(c.x), l.y.min(c.y)), Vec2::new(h.x.max(c.x), h.y.max(c.y)))
    });
    msg.push_str(&format!("; covers E {:.2} to {:.2}, N {:.2} to {:.2}", lo.x, hi.x, lo.y, hi.y));
    s.echo(msg.clone());
    let warning = cadcraft_render::image_layer::problem(&stored);
    if let Some(w) = &warning {
        s.echo(format!("WARNING: the picture is placed but cannot be shown: {w}"));
    }
    Ok(json!({
        "handle": handle.hex(),
        "size": [w, h],
        "worldFile": world.as_ref().map(|(_, f)| f),
        "warning": warning,
        "corners": corners.iter().map(|c| [c.x, c.y]).collect::<Vec<_>>(),
        "message": msg,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_file_places_the_lower_left_corner() {
        // 100 × 50 px north-up image, 0.5 m pixels, top-left pixel centre at (1000.25, 2000.75).
        let w = parse_world("0.5\n0\n0\n-0.5\n1000.25\n2000.75\n").unwrap();
        let (ll, u, v) = place_world(w, 50.0);
        assert_eq!((u, v), (Vec2::new(0.5, 0.0), Vec2::new(0.0, 0.5)));
        // Upper-left corner (1000, 2001); the image is 25 m tall, so the lower-left is (1000, 1976).
        assert!((ll.x - 1000.0).abs() < 1e-9 && (ll.y - 1976.0).abs() < 1e-9, "{ll:?}");
    }

    #[test]
    fn rotated_world_file_keeps_the_pixel_vectors() {
        let w = parse_world("0.4\n0.3\n0.3\n-0.4\n10\n20\n").unwrap();
        let (_, u, v) = place_world(w, 10.0);
        assert_eq!((u, v), (Vec2::new(0.4, 0.3), Vec2::new(-0.3, 0.4)));
    }

    #[test]
    fn bad_world_files_are_refused() {
        assert!(parse_world("1\n0\n0\n-1\n5\n").is_err(), "five numbers");
        assert!(parse_world("1\n0\n0\nx\n5\n6\n").is_err(), "not a number");
        assert!(parse_world("0\n0\n0\n0\n5\n6\n").is_err(), "zero pixel size");
        assert!(parse_world("NaN\n0\n0\n-1\n5\n6\n").is_err(), "not finite");
    }

    #[test]
    fn world_file_names_follow_the_image_type() {
        let c = world_candidates("/data/site.jpg");
        assert_eq!(c.first().map(String::as_str), Some("/data/site.jgw"));
        assert!(c.contains(&"/data/site.wld".to_string()));
        assert!(world_candidates("/a.b/site").contains(&"/a.b/site.wld".to_string()));
    }
}
