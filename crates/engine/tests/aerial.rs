//! AERIAL: aerial photos placed by coordinates, from tiles served by a stand-in for the network.

use std::sync::{Arc, Mutex};

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use cadcraft_engine::cmd::aerial::TileFetcher;
use cadcraft_geom::Crs;
use serde_json::json;

/// Serves a 256 x 256 PNG for every tile: red and green are the pixel's place in the tile, blue
/// names the tile, so a picture shows which tile and which pixel every output pixel came from.
struct Fake {
    calls: Mutex<Vec<String>>,
    fail: Box<dyn Fn(i64, i64, i64) -> bool + Send + Sync>,
}

fn tile_id(x: i64, y: i64) -> u8 {
    ((x * 37 + y * 91).rem_euclid(251)) as u8
}

impl TileFetcher for Fake {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.calls.lock().map_err(|e| e.to_string())?.push(url.to_string());
        let nums: Vec<i64> = url.split(['/', '.']).filter_map(|t| t.parse().ok()).collect();
        let [z, y, x] = nums[nums.len() - 3..] else { return Err("bad url".into()) };
        if (self.fail)(z, x, y) {
            return Err("404".into());
        }
        let mut img = image::RgbImage::new(256, 256);
        for (px, py, c) in img.enumerate_pixels_mut() {
            *c = image::Rgb([px as u8, py as u8, tile_id(x, y)]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(out.into_inner())
    }
}

fn session(fail: impl Fn(i64, i64, i64) -> bool + Send + Sync + 'static) -> (Session, Arc<Fake>) {
    let fake = Arc::new(Fake { calls: Mutex::new(Vec::new()), fail: Box::new(fail) });
    let mut s = Session::new();
    s.tile_fetcher = Some(fake.clone());
    (s, fake)
}

fn out_path(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("cadcraft-aerial-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().into_owned()
}

const BANGKOK: [f64; 4] = [660_000.0, 1_520_000.0, 662_000.0, 1_522_000.0];

#[test]
fn the_photo_lands_where_the_coordinates_say() {
    let (mut s, fake) = session(|_, _, _| false);
    let path = out_path("georef.png");
    let r = s.execute("aerial", &json!({ "crs": "EPSG:32647", "extent": BANGKOK, "resolution": 20, "path": path })).unwrap();
    assert_eq!(r["epsg"], 32647);
    assert_eq!(r["zoom"], 13);
    assert_eq!(r["size"], json!([100, 100]));
    assert_eq!(r["missingTiles"], 0);
    assert_eq!(fake.calls.lock().unwrap().len() as u64, r["tiles"].as_u64().unwrap());
    assert!(fake.calls.lock().unwrap()[0].starts_with("https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless_3857/default/g/13/"));

    // The world file: pixel size, north-up, the centre of the top-left pixel.
    let pgw = std::fs::read_to_string(path.replace(".png", ".pgw")).unwrap();
    let v: Vec<f64> = pgw.lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(v, vec![20.0, 0.0, 0.0, -20.0, 660_010.0, 1_521_990.0]);
    assert!(std::fs::read_to_string(path.replace(".png", ".prj")).unwrap().contains("UTM zone 47N"));

    // The image object covers the extent.
    let img = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None }).unwrap();
    assert_eq!((img.insert.x, img.insert.y), (660_000.0, 1_520_000.0));
    assert_eq!((img.u.x * img.size.x, img.v.y * img.size.y), (2000.0, 2000.0));

    // Every pixel shows the tile and tile position its coordinates fall on.
    let png = image::open(&path).unwrap().to_rgba8();
    let utm = Crs::Utm { zone: 47, north: true };
    let world = 256.0 * f64::from(1u32 << 13);
    let mut checked = 0;
    for (i, j) in [(5u32, 5u32), (50, 50), (95, 10), (10, 95), (73, 31), (30, 80)] {
        let (x, y) = (660_000.0 + (f64::from(i) + 0.5) * 20.0, 1_522_000.0 - (f64::from(j) + 0.5) * 20.0);
        let (lon, lat) = utm.to_lonlat(x, y).unwrap();
        let gx = (lon + 180.0) / 360.0 * world;
        let gy = (0.5 - ((1.0 + lat.to_radians().sin()) / (1.0 - lat.to_radians().sin())).ln() / (4.0 * std::f64::consts::PI)) * world;
        let (tx, ty) = ((gx / 256.0).floor() as i64, (gy / 256.0).floor() as i64);
        let (lx, ly) = (gx - 256.0 * tx as f64 - 0.5, gy - 256.0 * ty as f64 - 0.5);
        if !(2.0..253.0).contains(&lx) || !(2.0..253.0).contains(&ly) {
            continue; // blending across a tile seam
        }
        let p = png.get_pixel(i, j).0;
        assert_eq!(p[3], 255);
        assert!((f64::from(p[0]) - lx).abs() <= 1.0 && (f64::from(p[1]) - ly).abs() <= 1.0, "pixel {i},{j}: got {p:?}, expected {lx:.1},{ly:.1}");
        assert_eq!(p[2], tile_id(tx, ty), "pixel {i},{j} comes from tile {tx},{ty}");
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} pixels were far from a tile seam");

    // The credit is drawn, and the drawing remembers its coordinate system.
    assert!(s.doc().unwrap().model.iter().any(|e| e.common.layer == "AERIAL-CREDIT"));
    assert!(s.doc().unwrap().model.iter().any(|e| e.common.layer == "AERIAL"));
    assert_eq!(s.doc().unwrap().header.str("USERS5", ""), "EPSG:32647");
    let r2 = s.execute("aerial", &json!({ "extent": BANGKOK, "resolution": 20, "path": out_path("again.png"), "attribution": false })).unwrap();
    assert_eq!(r2["epsg"], 32647, "the system comes from the drawing");
}

#[test]
fn a_longitude_latitude_box_picks_the_zone_and_the_drawing_extent_is_the_default() {
    let (mut s, _) = session(|_, _, _| false);
    let r = s.execute("aerial", &json!({ "lonLat": [100.49, 13.74, 100.51, 13.76], "resolution": 10, "path": out_path("box.png") })).unwrap();
    assert_eq!(r["crs"], "WGS 84 / UTM zone 47N");
    let e = r["extent"].as_array().unwrap();
    assert!(e[0].as_f64().unwrap() > 660_000.0 && e[2].as_f64().unwrap() < 665_000.0, "{e:?}");
    // With a drawing in place, the area is the drawing's, a little larger.
    let (mut t, _) = session(|_, _, _| false);
    t.execute("line", &json!({ "points": [[660_000, 1_520_000], [661_000, 1_521_000]] })).unwrap();
    let r = t.execute("aerial", &json!({ "crs": "UTM 47N", "resolution": 20, "path": out_path("dwg.png") })).unwrap();
    let e = r["extent"].as_array().unwrap();
    assert!(e[0].as_f64().unwrap() < 660_000.0 && e[2].as_f64().unwrap() > 661_000.0);
    // The Web Mercator system works too, and a southern zone.
    let (mut w, _) = session(|_, _, _| false);
    let r = w
        .execute(
            "aerial",
            &json!({ "crs": "EPSG:3857", "extent": [11_180_000, 1_540_000, 11_182_000, 1_542_000], "resolution": 20, "path": out_path("wm.png") }),
        )
        .unwrap();
    assert_eq!(r["epsg"], 3857);
    let r = w
        .execute("aerial", &json!({ "crs": "UTM 56S", "lonLat": [151.2, -33.9, 151.22, -33.88], "resolution": 20, "path": out_path("syd.png") }))
        .unwrap();
    assert_eq!(r["epsg"], 32756);
}

#[test]
fn missing_tiles_leave_the_picture_empty_there_and_a_dead_service_is_an_error() {
    let (mut s, _) = session(|_, x, _| x % 2 == 0);
    let r = s
        .execute(
            "aerial",
            &json!({ "crs": "UTM 47N", "extent": [650_000, 1_510_000, 670_000, 1_530_000], "resolution": 20, "path": out_path("holes.png") }),
        )
        .unwrap();
    let missing = r["missingTiles"].as_u64().unwrap();
    assert!(missing > 0 && missing < r["tiles"].as_u64().unwrap(), "{r}");
    let png = image::open(r["path"].as_str().unwrap()).unwrap().to_rgba8();
    assert!(png.pixels().any(|p| p.0[3] == 0) && png.pixels().any(|p| p.0[3] == 255));

    let (mut dead, _) = session(|_, _, _| true);
    let n = dead.doc().unwrap().model.iter().count();
    let e = dead
        .execute("aerial", &json!({ "crs": "UTM 47N", "extent": BANGKOK, "resolution": 20, "path": out_path("dead.png") }))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no tile could be downloaded"), "{e}");
    assert_eq!(dead.doc().unwrap().model.iter().count(), n);
}

#[test]
fn too_many_tiles_lower_the_zoom() {
    let (mut s, fake) = session(|_, _, _| false);
    // Empire State Building area, UTM 18N: 2.4 km at 0.5 m per pixel would take about 480 tiles at zoom 18.
    let r = s
        .execute("aerial", &json!({ "source": "usgs-imagery", "crs": "UTM 18N", "extent": [584_000, 4_510_000, 586_400, 4_512_400], "resolution": 0.5, "path": out_path("many.png") }))
        .unwrap();
    assert_eq!(r["zoom"], 17, "{r}");
    assert!(r["note"].as_str().unwrap().contains("coarser"));
    assert!(fake.calls.lock().unwrap().len() <= 400);
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let (mut s, fake) = session(|_, _, _| false);
    let n = s.doc().unwrap().model.iter().count();
    let ok = json!({ "crs": "UTM 47N", "extent": BANGKOK, "resolution": 20, "path": out_path("never.png") });
    for bad in [
        json!({"source": "google"}),
        json!({"crs": "UTM 99N"}),
        json!({"crs": "EPSG:4326"}),
        json!({"crs": null, "lonLat": null, "extent": BANGKOK}),
        json!({"extent": [662_000, 1_520_000, 660_000, 1_522_000]}),
        json!({"extent": [1, 2, 3]}),
        json!({"extent": "x"}),
        json!({"resolution": 0}),
        json!({"resolution": -3}),
        json!({"resolution": 0.001}),
        json!({"source": "usgs-imagery"}),
        json!({"extent": [0, 0, 1e7, 1e7]}),
        json!({"extent": [660_000, 1_520_000, 660_000, 1_522_000]}),
        json!({"lonLat": [100, 13, 99, 14], "extent": null}),
        json!({"lonLat": [-200, 0, 10, 10]}),
        json!({"crs": "UTM 47N", "extent": [1e300, 1e300, 1e301, 1e301]}),
    ] {
        let mut p = ok.clone();
        for (k, v) in bad.as_object().unwrap() {
            p[k] = v.clone();
        }
        if bad.get("crs") == Some(&serde_json::Value::Null) {
            p.as_object_mut().unwrap().remove("crs");
            p.as_object_mut().unwrap().remove("lonLat");
        }
        assert!(s.execute("aerial", &p).is_err(), "{bad}");
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), n);
    assert!(fake.calls.lock().unwrap().is_empty(), "nothing was downloaded for a refused request");
    // The web version has no downloader.
}
