//! Raster images (aerial photos, scanned plans) on the drawing canvas: each IMAGE in a display
//! list is painted as a textured quad under the line work, honouring its rotation, clip polygon
//! and fade.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cadcraft_geom::Vec2;
use cadcraft_render::{DisplayList, ImagePlacement, image_layer};
use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Shape, TextureHandle, TextureOptions, pos2};

use crate::canvas::Xf;

/// Longest side of an uploaded texture (every GPU accepts this); larger photos are shrunk.
const MAX_TEXTURE_SIDE: u32 = 4096;
/// Textures kept between frames.
const MAX_TEXTURES: usize = 8;

type Entry = (u64, Option<TextureHandle>);

/// Textures by image path, shared by every canvas through the egui context.
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<String, Entry>>>);

fn texture(ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(egui::Id::new("cadcraft-image-textures")).clone());
    let stamp = image_layer::file_stamp(path);
    let mut map = cache.0.lock().ok()?;
    if let Some((s, t)) = map.get(path)
        && *s == stamp
    {
        return t.clone();
    }
    let tex = image_layer::display_rgba(path, MAX_TEXTURE_SIDE).and_then(|(w, h, px)| {
        let size = [usize::try_from(w).ok()?, usize::try_from(h).ok()?];
        (px.len() == size[0].checked_mul(size[1])?.checked_mul(4)?)
            .then(|| ctx.load_texture(format!("image:{path}"), egui::ColorImage::from_rgba_premultiplied(size, &px), TextureOptions::LINEAR))
    });
    if map.len() >= MAX_TEXTURES {
        map.clear();
    }
    map.insert(path.to_string(), (stamp, tex.clone()));
    tex
}

/// Texture coordinates of world point `q` on `im`.
fn uv_of(im: &ImagePlacement, q: Vec2) -> Option<[f32; 2]> {
    let (w, h) = (im.u * im.size.x, im.v * im.size.y);
    let det = w.x * h.y - w.y * h.x;
    if det.abs() < 1e-300 {
        return None;
    }
    let r = q - im.origin;
    let s = (r.x * h.y - r.y * h.x) / det;
    let t = (w.x * r.y - w.y * r.x) / det;
    Some([s as f32, (1.0 - t) as f32])
}

fn convex(poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut sign = 0.0f64;
    for i in 0..n {
        let (Some(a), Some(b), Some(c)) = (poly.get(i), poly.get((i + 1) % n), poly.get((i + 2) % n)) else { return false };
        let z = (*b - *a).cross(*c - *b);
        if z.abs() < 1e-18 {
            continue;
        }
        if sign == 0.0 {
            sign = z;
        } else if sign * z < 0.0 {
            return false;
        }
    }
    true
}

/// Paint the images of `list` through `p`, in list order. Call before the line work.
pub(crate) fn draw(p: &egui::Painter, xf: &Xf, list: &DisplayList) {
    if list.images.is_empty() {
        return;
    }
    let vis = xf.world_bounds();
    for im in &list.images {
        let corners = im.corners();
        if im.alpha == 0 || !cadcraft_geom::Bounds2::from_points(corners.iter().copied()).intersects(&vis) {
            continue;
        }
        let Some(tex) = texture(p.ctx(), &im.path) else { continue };
        // A convex clip polygon is the fan outline; anything else falls back to the whole image.
        let outline: &[Vec2] = if im.clip.len() >= 3 && convex(&im.clip) { &im.clip } else { &corners };
        let tint = Color32::from_white_alpha(im.alpha);
        let mut mesh = Mesh::with_texture(tex.id());
        for q in outline {
            let Some(uv) = uv_of(im, *q) else { continue };
            mesh.vertices.push(Vertex { pos: xf.to_screen(*q), uv: pos2(uv[0], uv[1]), color: tint });
        }
        let n = u32::try_from(mesh.vertices.len()).unwrap_or(0);
        if n < 3 {
            continue;
        }
        for i in 1..n - 1 {
            mesh.indices.extend_from_slice(&[0, i, i + 1]);
        }
        p.add(Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement() -> ImagePlacement {
        ImagePlacement {
            handle: cadcraft_doc::Handle(1),
            path: String::new(),
            origin: Vec2::new(10.0, 20.0),
            u: Vec2::new(0.0, 0.5),
            v: Vec2::new(-0.5, 0.0),
            size: Vec2::new(100.0, 40.0),
            clip: Vec::new(),
            alpha: 255,
        }
    }

    #[test]
    fn texture_coordinates_follow_a_rotated_image() {
        let im = placement();
        let c = im.corners();
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        // Lower-left, lower-right, upper-right, upper-left of the picture.
        let want = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (q, w) in c.iter().zip(want) {
            assert!(near(uv_of(&im, *q).unwrap_or([9.0, 9.0]), w), "{q:?}");
        }
    }

    #[test]
    fn convexity() {
        let sq = [Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)];
        assert!(convex(&sq));
        let dart = [Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 4.0)];
        assert!(!convex(&dart));
    }
}
