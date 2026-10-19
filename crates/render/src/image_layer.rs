//! Raster images placed in a drawing (IMAGE entities, such as aerial photos and scanned plans):
//! where each one sits ([`ImagePlacement`], recorded in the display list) and drawing its pixels
//! (the CPU rasteriser draws images under the line work).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cadcraft_doc::Handle;
use cadcraft_geom::{Bounds2, Vec2};
use tiny_skia::{FillRule, FilterQuality, Mask, Paint, PathBuilder, Pattern, Pixmap, SpreadMode, Transform};

use crate::raster::View;

/// Most pixels decoded from one image file (a corrupt header can claim anything).
const MAX_PIXELS: u64 = 100_000_000;
/// Decoded images kept between frames.
const MAX_CACHED: usize = 8;

/// One image in a display list, in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ImagePlacement {
    pub handle: Handle,
    /// The image file.
    pub path: String,
    /// Lower-left corner of the image.
    pub origin: Vec2,
    /// One pixel's width and height (up the image) vectors.
    pub u: Vec2,
    pub v: Vec2,
    /// Size in pixels as stored in the drawing.
    pub size: Vec2,
    /// Clip polygon in world coordinates; empty draws the whole image.
    pub clip: Vec<Vec2>,
    /// Opacity, 0..=255.
    pub alpha: u8,
}

impl ImagePlacement {
    /// The four corners, counter-clockwise from the lower-left.
    pub fn corners(&self) -> [Vec2; 4] {
        let (w, h) = (self.u * self.size.x, self.v * self.size.y);
        [self.origin, self.origin + w, self.origin + w + h, self.origin + h]
    }
}

/// The image file exists (always false on the web, which has no files).
pub fn available(path: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        !path.is_empty() && std::path::Path::new(path).is_file()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        false
    }
}

type Cache = Mutex<HashMap<String, (u64, Option<Arc<Pixmap>>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A change stamp of the file (modification time and size), so an edited photo is read again.
#[cfg(not(target_arch = "wasm32"))]
fn stamp(path: &str) -> u64 {
    let Ok(m) = std::fs::metadata(path) else { return 0 };
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    t.wrapping_mul(1_000_003).wrapping_add(m.len())
}

/// Longest side wanted from a JPEG too big to decode whole (see [`decode_jpeg_small`]).
#[cfg(not(target_arch = "wasm32"))]
const BIG_JPEG_TARGET: u16 = 3000;

/// A very large JPEG (a survey orthophoto can be 25 000 px square) decoded at 1/2, 1/4 or 1/8 of
/// its size, as the JPEG format allows, so the full picture never sits in memory. The drawing
/// stretches the result over the image's frame. `None` for formats other than grey and RGB.
#[cfg(not(target_arch = "wasm32"))]
fn decode_jpeg_small(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    let mut d = jpeg_decoder::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    d.read_info().ok()?;
    let (w, h) = d.scale(BIG_JPEG_TARGET, BIG_JPEG_TARGET).ok()?;
    let px = d.decode().ok()?;
    let n = usize::from(w).checked_mul(usize::from(h))?;
    let mut out = Vec::with_capacity(n.checked_mul(4)?);
    match d.info()?.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 if px.len() == n.checked_mul(3)? => {
            for c in px.chunks_exact(3) {
                out.extend_from_slice(c);
                out.push(255);
            }
        }
        jpeg_decoder::PixelFormat::L8 if px.len() == n => {
            for g in px {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        _ => return None,
    }
    Some((u32::from(w), u32::from(h), out))
}

#[cfg(not(target_arch = "wasm32"))]
fn decode(path: &str) -> Option<Pixmap> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let jpeg = reader.format() == Some(image::ImageFormat::Jpeg);
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let (w, h, mut data) = if u64::from(w) * u64::from(h) > MAX_PIXELS {
        if !jpeg {
            return None;
        }
        decode_jpeg_small(path)?
    } else {
        let rgba = image::open(path).ok()?.into_rgba8();
        let (w, h) = rgba.dimensions();
        (w, h, rgba.into_raw())
    };
    // tiny-skia holds premultiplied colour.
    for px in data.chunks_exact_mut(4) {
        if let [r, g, b, a] = px {
            let a32 = u32::from(*a);
            if a32 != 255 {
                *r = u8::try_from((u32::from(*r) * a32 + 127) / 255).unwrap_or(u8::MAX);
                *g = u8::try_from((u32::from(*g) * a32 + 127) / 255).unwrap_or(u8::MAX);
                *b = u8::try_from((u32::from(*b) * a32 + 127) / 255).unwrap_or(u8::MAX);
            }
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// The decoded image at `path`, from the cache when the file hasn't changed. `None` when the
/// file can't be read as an image.
pub(crate) fn load(path: &str) -> Option<Arc<Pixmap>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let st = stamp(path);
        let mut c = cache().lock().ok()?;
        if let Some((s, p)) = c.get(path)
            && *s == st
        {
            return p.clone();
        }
        let pm = decode(path).map(Arc::new);
        if c.len() >= MAX_CACHED {
            c.clear();
        }
        c.insert(path.to_string(), (st, pm.clone()));
        pm
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, cache());
        None
    }
}

/// Why the image at `path` cannot be shown, in words for the user; `None` when it loads.
pub fn problem(path: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if load(path).is_some() {
            return None;
        }
        let reader = match image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
            Ok(r) => r,
            Err(e) => return Some(format!("the file cannot be read ({e})")),
        };
        let jpeg = reader.format() == Some(image::ImageFormat::Jpeg);
        let (w, h) = match reader.into_dimensions() {
            Ok(d) => d,
            Err(e) => return Some(format!("not a readable PNG, JPEG or TIFF picture ({e})")),
        };
        if u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Some(if jpeg {
                format!("a {w} × {h} JPEG of this kind (not RGB or grey) is too large to decode; save it as a normal RGB JPEG")
            } else {
                format!("{w} × {h} pixels is larger than {} million pixels; only JPEG files that big can be shown", MAX_PIXELS / 1_000_000)
            });
        }
        Some(match image::open(path) {
            Err(e) => format!("the picture cannot be decoded ({e})"),
            Ok(_) => "the picture could not be loaded".to_string(),
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Some("pictures cannot be shown on the web".to_string())
    }
}

/// A change stamp of the image file (0 when it can't be read, and on the web).
pub fn file_stamp(path: &str) -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stamp(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        0
    }
}

/// The image at `path` for a GPU texture: premultiplied RGBA8 `(width, height, pixels)`, shrunk
/// by block averaging until neither side exceeds `max_side`. `None` when it can't be read.
pub fn display_rgba(path: &str, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = load(path)?;
    let (w, h) = (img.width(), img.height());
    let step = w.max(h).div_ceil(max_side.max(1)).max(1);
    if step == 1 {
        return Some((w, h, img.data().to_vec()));
    }
    let (nw, nh) = (w.div_ceil(step), h.div_ceil(step));
    let src = img.data();
    let mut out = Vec::with_capacity((nw as usize).saturating_mul(nh as usize).saturating_mul(4));
    for oy in 0..nh {
        for ox in 0..nw {
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for y in (oy * step)..((oy + 1) * step).min(h) {
                for x in (ox * step)..((ox + 1) * step).min(w) {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    if let Some(px) = src.get(i..i + 4) {
                        for (acc, v) in sum.iter_mut().zip(px) {
                            *acc += u32::from(*v);
                        }
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.extend(sum.iter().map(|v| u8::try_from(v / n).unwrap_or(u8::MAX)));
        }
    }
    Some((nw, nh, out))
}

/// Draw one image onto `pm` under `view`.
pub(crate) fn draw(pm: &mut Pixmap, im: &ImagePlacement, view: &View, antialias: bool) {
    let corners = im.corners();
    if !Bounds2::from_points(corners.iter().copied()).intersects(&view.world_bounds()) {
        return;
    }
    let Some(img) = load(&im.path) else { return };
    let (pw, ph) = (img.width() as f32, img.height() as f32);
    if im.size.x <= 0.0 || im.size.y <= 0.0 {
        return;
    }
    // The file may differ in size from what the drawing recorded: it still fills its frame.
    let u = im.u * (im.size.x / f64::from(pw));
    let v = im.v * (im.size.y / f64::from(ph));
    let upper_left = im.origin + im.v * im.size.y;
    let (ax, ay) = view.to_screen(upper_left);
    let (bx, by) = view.to_screen(upper_left + u);
    let (cx, cy) = view.to_screen(upper_left - v);
    // Pixel (column, row down) -> screen.
    let tf = Transform::from_row(bx - ax, by - ay, cx - ax, cy - ay, ax, ay);
    let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, pw, ph) else { return };

    let shader = Pattern::new(Pixmap::as_ref(&img), SpreadMode::Pad, FilterQuality::Bilinear, f32::from(im.alpha) / 255.0, Transform::identity());
    let paint = Paint { shader, anti_alias: antialias, ..Paint::default() };

    let mask = if im.clip.len() >= 3 {
        let mut pb = PathBuilder::new();
        for (i, q) in im.clip.iter().enumerate() {
            let (x, y) = view.to_screen(*q);
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        pb.close();
        match (pb.finish(), Mask::new(pm.width(), pm.height())) {
            (Some(path), Some(mut m)) => {
                m.fill_path(&path, FillRule::Winding, antialias, Transform::identity());
                Some(m)
            }
            _ => return,
        }
    } else {
        None
    };
    pm.fill_path(&PathBuilder::from_rect(rect), &paint, FillRule::Winding, tf, mask.as_ref());
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_big_jpeg_is_decoded_at_a_fraction_of_its_size() {
        let path = std::env::temp_dir().join(format!("cc-bigjpeg-{}.jpg", std::process::id()));
        // 6400 × 3200 px, solid orange: too big to need full size, small enough to write quickly.
        image::RgbImage::from_pixel(6400, 3200, image::Rgb([240, 120, 10])).save(&path).unwrap();
        let (w, h, px) = decode_jpeg_small(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(w < 6400 && h < 3200 && w >= 3000 / 2, "{w} × {h}");
        assert_eq!(px.len(), w as usize * h as usize * 4);
        let mid = (h as usize / 2 * w as usize + w as usize / 2) * 4;
        let p = &px[mid..mid + 4];
        assert!((i32::from(p[0]) - 240).abs() < 8 && (i32::from(p[1]) - 120).abs() < 8 && (i32::from(p[2]) - 10).abs() < 12, "{p:?}");
    }
}
