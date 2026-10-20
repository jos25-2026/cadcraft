//! Coordinate reference systems for placing aerial photos: WGS 84 longitude/latitude, Web
//! Mercator and WGS 84 UTM (every zone, north or south). The transverse Mercator uses Krüger's
//! series in the third flattening n to the sixth order (Karney 2011, published formulas),
//! accurate far below a millimetre inside a zone.
//!
//! Every function returns `None` for non-finite input or a point the projection cannot reach, and
//! never panics: positions come from files and command parameters.

use std::f64::consts::PI;

/// WGS 84 semi-major axis (m) and flattening.
const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;
/// UTM scale on the central meridian, false easting and the southern false northing.
const K0: f64 = 0.9996;
const E0: f64 = 500_000.0;
const N0_SOUTH: f64 = 10_000_000.0;
/// Latitude limit of Web Mercator (degrees).
pub const MERCATOR_MAX_LAT: f64 = 85.051_128_779_806_59;

/// A coordinate system a drawing can be in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Crs {
    /// EPSG:4326, `x` = longitude and `y` = latitude, in degrees.
    LonLat,
    /// EPSG:3857, metres.
    WebMercator,
    /// WGS 84 / UTM zone `zone` (1 to 60), metres; EPSG:326xx north, 327xx south.
    Utm { zone: u8, north: bool },
}

impl Crs {
    /// Reads `EPSG:32647`, `32647`, `UTM 47N`, `WGS84 UTM 47 N`, `47N`, `47S`, `EPSG:4326`, `EPSG:3857`,
    /// `lonlat` or `mercator`.
    pub fn parse(text: &str) -> Option<Crs> {
        let t = text.trim().to_ascii_uppercase();
        let t = t.trim_start_matches("EPSG:").trim_start_matches("EPSG").trim();
        match t {
            "4326" | "LONLAT" | "WGS84" | "LATLON" | "LATLONG" => return Some(Crs::LonLat),
            "3857" | "MERCATOR" | "WEBMERCATOR" | "900913" => return Some(Crs::WebMercator),
            _ => {}
        }
        if let Ok(code) = t.parse::<u32>() {
            return match code {
                32601..=32660 => Some(Crs::Utm { zone: u8::try_from(code - 32600).ok()?, north: true }),
                32701..=32760 => Some(Crs::Utm { zone: u8::try_from(code - 32700).ok()?, north: false }),
                1..=60 => Some(Crs::Utm { zone: u8::try_from(code).ok()?, north: true }),
                _ => None,
            };
        }
        let t = t.trim_start_matches("WGS84").trim_start_matches("WGS 84").trim().trim_start_matches('/').trim();
        let t = t.trim_start_matches("UTM").trim().trim_start_matches("ZONE").trim();
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = t[digits.len()..].trim();
        let zone = digits.parse::<u8>().ok().filter(|z| (1..=60).contains(z))?;
        let north = match rest {
            "N" | "NORTH" | "" => true,
            "S" | "SOUTH" => false,
            _ => return None,
        };
        Some(Crs::Utm { zone, north })
    }

    /// The EPSG code, e.g. 32647.
    pub fn epsg(self) -> u32 {
        match self {
            Crs::LonLat => 4326,
            Crs::WebMercator => 3857,
            Crs::Utm { zone, north } => (if north { 32600 } else { 32700 }) + u32::from(zone),
        }
    }

    /// A readable name: `WGS 84 / UTM zone 47N`.
    pub fn name(self) -> String {
        match self {
            Crs::LonLat => "WGS 84".to_string(),
            Crs::WebMercator => "WGS 84 / Pseudo-Mercator".to_string(),
            Crs::Utm { zone, north } => format!("WGS 84 / UTM zone {zone}{}", if north { "N" } else { "S" }),
        }
    }

    /// Longitude of the central meridian of a UTM zone (degrees).
    pub fn central_meridian(zone: u8) -> f64 {
        f64::from(zone) * 6.0 - 183.0
    }

    /// The zone (1 to 60) and hemisphere that contain a longitude and latitude (degrees).
    pub fn utm_for(lon: f64, lat: f64) -> Option<Crs> {
        if !(lon.is_finite() && lat.is_finite() && (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)) {
            return None;
        }
        let zone = (((lon + 180.0) / 6.0).floor() as i64 + 1).clamp(1, 60) as u8;
        Some(Crs::Utm { zone, north: lat >= 0.0 })
    }

    /// Longitude and latitude (degrees) to this system's coordinates.
    pub fn from_lonlat(self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        if !(lon.is_finite() && lat.is_finite() && lat.abs() <= 90.0) {
            return None;
        }
        match self {
            Crs::LonLat => Some((lon, lat)),
            Crs::WebMercator => {
                if lat.abs() > MERCATOR_MAX_LAT {
                    return None;
                }
                let y = A * (PI / 4.0 + lat.to_radians() / 2.0).tan().ln();
                Some((A * lon.to_radians(), y))
            }
            Crs::Utm { zone, north } => utm_forward(zone, north, lon, lat),
        }
    }

    /// This system's coordinates to longitude and latitude (degrees).
    pub fn to_lonlat(self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        match self {
            Crs::LonLat => (x.abs() <= 360.0 && y.abs() <= 90.0).then_some((x, y)),
            Crs::WebMercator => {
                let lat = (2.0 * (y / A).exp().atan() - PI / 2.0).to_degrees();
                let lon = (x / A).to_degrees();
                (lat.is_finite() && lon.is_finite()).then_some((lon, lat))
            }
            Crs::Utm { zone, north } => utm_inverse(zone, north, x, y),
        }
    }

    /// A WKT description for a `.prj` file.
    pub fn wkt(self) -> String {
        let geog = "GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]]";
        match self {
            Crs::LonLat => geog.to_string(),
            Crs::WebMercator => format!(
                "PROJCS[\"WGS 84 / Pseudo-Mercator\",{geog},PROJECTION[\"Mercator_1SP\"],PARAMETER[\"central_meridian\",0],PARAMETER[\"scale_factor\",1],PARAMETER[\"false_easting\",0],PARAMETER[\"false_northing\",0],UNIT[\"metre\",1]]"
            ),
            Crs::Utm { zone, north } => format!(
                "PROJCS[\"{}\",{geog},PROJECTION[\"Transverse_Mercator\"],PARAMETER[\"latitude_of_origin\",0],PARAMETER[\"central_meridian\",{}],PARAMETER[\"scale_factor\",0.9996],PARAMETER[\"false_easting\",500000],PARAMETER[\"false_northing\",{}],UNIT[\"metre\",1]]",
                self.name(),
                Crs::central_meridian(zone),
                if north { 0 } else { 10_000_000 }
            ),
        }
    }

    /// Whether the unit is degrees (a drawing in degrees cannot hold a photo to scale).
    pub fn is_geographic(self) -> bool {
        matches!(self, Crs::LonLat)
    }
}

/// The third flattening, the rectifying radius and Krüger's coefficients.
struct Kruger {
    a_hat: f64,
    alpha: [f64; 6],
    beta: [f64; 6],
    delta: [f64; 6],
    n: f64,
}

fn kruger() -> Kruger {
    let n = F / (2.0 - F);
    let (n2, n3, n4, n5, n6) = (n * n, n.powi(3), n.powi(4), n.powi(5), n.powi(6));
    Kruger {
        n,
        a_hat: A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0),
        alpha: [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0 + 7891.0 * n6 / 37800.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0 - 1_983_433.0 * n6 / 1_935_360.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0 + 167_603.0 * n6 / 181_440.0,
            49561.0 * n4 / 161_280.0 - 179.0 * n5 / 168.0 + 6_601_661.0 * n6 / 7_257_600.0,
            34729.0 * n5 / 80640.0 - 3_418_889.0 * n6 / 1_995_840.0,
            212_378_941.0 * n6 / 319_334_400.0,
        ],
        beta: [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0 + 96199.0 * n6 / 604_800.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0 - 1_118_711.0 * n6 / 3_870_720.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0 + 5569.0 * n6 / 90720.0,
            4397.0 * n4 / 161_280.0 - 11.0 * n5 / 504.0 - 830_251.0 * n6 / 7_257_600.0,
            4583.0 * n5 / 161_280.0 - 108_847.0 * n6 / 3_991_680.0,
            20_648_693.0 * n6 / 638_668_800.0,
        ],
        delta: [
            2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0 + 26.0 * n5 / 45.0 - 2854.0 * n6 / 675.0,
            7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0 + 2704.0 * n5 / 315.0 + 2323.0 * n6 / 945.0,
            56.0 * n3 / 15.0 - 136.0 * n4 / 35.0 - 1262.0 * n5 / 105.0 + 73814.0 * n6 / 2835.0,
            4279.0 * n4 / 630.0 - 332.0 * n5 / 35.0 - 399_572.0 * n6 / 14175.0,
            4174.0 * n5 / 315.0 - 144_838.0 * n6 / 6237.0,
            601_676.0 * n6 / 22275.0,
        ],
    }
}

fn utm_forward(zone: u8, north: bool, lon: f64, lat: f64) -> Option<(f64, f64)> {
    if !(1..=60).contains(&zone) {
        return None;
    }
    let k = kruger();
    let dl = (lon - Crs::central_meridian(zone)).to_radians();
    // Far outside the zone the series diverge; refuse beyond 40 degrees from the meridian.
    if dl.abs() > 40.0_f64.to_radians() {
        return None;
    }
    let phi = lat.to_radians();
    let c = 2.0 * k.n.sqrt() / (1.0 + k.n);
    let t = (phi.sin().atanh() - c * (c * phi.sin()).atanh()).sinh();
    let xi0 = t.atan2(dl.cos());
    let eta0 = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut xi, mut eta) = (xi0, eta0);
    for (j, a) in k.alpha.iter().enumerate() {
        let m = 2.0 * (j + 1) as f64;
        xi += a * (m * xi0).sin() * (m * eta0).cosh();
        eta += a * (m * xi0).cos() * (m * eta0).sinh();
    }
    let e = E0 + K0 * k.a_hat * eta;
    let n = K0 * k.a_hat * xi + if north { 0.0 } else { N0_SOUTH };
    (e.is_finite() && n.is_finite()).then_some((e, n))
}

fn utm_inverse(zone: u8, north: bool, e: f64, n: f64) -> Option<(f64, f64)> {
    if !(1..=60).contains(&zone) {
        return None;
    }
    let k = kruger();
    let xi = (n - if north { 0.0 } else { N0_SOUTH }) / (K0 * k.a_hat);
    let eta = (e - E0) / (K0 * k.a_hat);
    // Outside a few thousand kilometres the series are meaningless.
    if xi.abs() > 1.7 || eta.abs() > 1.2 {
        return None;
    }
    let (mut xi0, mut eta0) = (xi, eta);
    for (j, b) in k.beta.iter().enumerate() {
        let m = 2.0 * (j + 1) as f64;
        xi0 -= b * (m * xi).sin() * (m * eta).cosh();
        eta0 -= b * (m * xi).cos() * (m * eta).sinh();
    }
    let chi = (xi0.sin() / eta0.cosh()).asin();
    let mut phi = chi;
    for (j, d) in k.delta.iter().enumerate() {
        phi += d * (2.0 * (j + 1) as f64 * chi).sin();
    }
    let lon = Crs::central_meridian(zone) + eta0.sinh().atan2(xi0.cos()).to_degrees();
    let lat = phi.to_degrees();
    (lon.is_finite() && lat.is_finite()).then_some((lon, lat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn the_central_meridian_and_equator_are_the_false_origin() {
        let utm = Crs::Utm { zone: 47, north: true };
        let (e, n) = utm.from_lonlat(99.0, 0.0).unwrap_or_default();
        assert!(near(e, 500_000.0, 1e-6) && near(n, 0.0, 1e-6), "{e} {n}");
        let south = Crs::Utm { zone: 47, north: false };
        let (e, n) = south.from_lonlat(99.0, -1.0).unwrap_or_default();
        assert!(near(e, 500_000.0, 1e-6) && near(n, 10_000_000.0 - 110_574.389 * 0.9996, 0.05), "{e} {n}");
        // On the central meridian the northing is the scaled meridian arc: 1 degree at the equator is 110574.4 m.
        let (_, n1) = utm.from_lonlat(99.0, 1.0).unwrap_or_default();
        assert!(near(n1, 110_574.389 * 0.9996, 0.05), "{n1}");
    }

    #[test]
    fn published_points_agree() {
        // The Empire State Building: 40.7484 N, 73.9857 W is 18T 585628 E, 4511322 N.
        let utm = Crs::Utm { zone: 18, north: true };
        let (e, n) = utm.from_lonlat(-73.9857, 40.7484).unwrap_or_default();
        assert!(near(e, 585_628.0, 1.0) && near(n, 4_511_322.0, 1.0), "{e} {n}");
        let (lon, lat) = utm.to_lonlat(e, n).unwrap_or_default();
        assert!(near(lon, -73.9857, 1e-7) && near(lat, 40.7484, 1e-7));
    }

    #[test]
    fn every_zone_round_trips_to_a_nanometre_of_a_degree() {
        for zone in 1..=60u8 {
            for north in [true, false] {
                let utm = Crs::Utm { zone, north };
                let cm = Crs::central_meridian(zone);
                for dlon in [-3.0, -1.0, 0.0, 2.5, 3.0] {
                    for lat in [0.0, 13.7, 45.0, 66.0, 80.0] {
                        let lat = if north { lat } else { -lat };
                        let Some((e, n)) = utm.from_lonlat(cm + dlon, lat) else { panic!("{zone} {dlon} {lat}") };
                        let (lon2, lat2) = utm.to_lonlat(e, n).unwrap_or_default();
                        assert!(near(lon2, cm + dlon, 1e-9) && near(lat2, lat, 1e-9), "zone {zone} {dlon} {lat}: {lon2} {lat2}");
                    }
                }
            }
        }
    }

    #[test]
    fn web_mercator_matches_its_definition() {
        let (x, y) = Crs::WebMercator.from_lonlat(180.0, 0.0).unwrap_or_default();
        assert!(near(x, 20_037_508.342_789_244, 1e-6) && near(y, 0.0, 1e-6));
        let (x, y) = Crs::WebMercator.from_lonlat(100.5018, 13.7563).unwrap_or_default();
        assert!(near(x, 11_187_000.0, 5_000.0) && near(y, 1_545_000.0, 5_000.0), "{x} {y}");
        let (lon, lat) = Crs::WebMercator.to_lonlat(x, y).unwrap_or_default();
        assert!(near(lon, 100.5018, 1e-9) && near(lat, 13.7563, 1e-9));
        assert!(Crs::WebMercator.from_lonlat(0.0, 89.0).is_none(), "beyond the Mercator limit");
    }

    #[test]
    fn names_codes_and_zones_parse() {
        assert_eq!(Crs::parse("EPSG:32647"), Some(Crs::Utm { zone: 47, north: true }));
        assert_eq!(Crs::parse("32748"), Some(Crs::Utm { zone: 48, north: false }));
        assert_eq!(Crs::parse("UTM 47N"), Some(Crs::Utm { zone: 47, north: true }));
        assert_eq!(Crs::parse("wgs84 utm 48 s"), Some(Crs::Utm { zone: 48, north: false }));
        assert_eq!(Crs::parse("WGS 84 / UTM zone 47N"), Some(Crs::Utm { zone: 47, north: true }));
        assert_eq!(Crs::parse("47"), Some(Crs::Utm { zone: 47, north: true }));
        assert_eq!(Crs::parse("EPSG:4326"), Some(Crs::LonLat));
        assert_eq!(Crs::parse("3857"), Some(Crs::WebMercator));
        for bad in ["", "UTM 61N", "UTM 0N", "EPSG:99999", "47X", "banana", "UTM -3N"] {
            assert_eq!(Crs::parse(bad), None, "{bad}");
        }
        assert_eq!(Crs::Utm { zone: 47, north: true }.epsg(), 32647);
        assert_eq!(Crs::utm_for(100.5, 13.7), Some(Crs::Utm { zone: 47, north: true }));
        assert_eq!(Crs::utm_for(-179.9, -5.0), Some(Crs::Utm { zone: 1, north: false }));
        assert_eq!(Crs::utm_for(180.0, 0.0), Some(Crs::Utm { zone: 60, north: true }));
        assert!(Crs::Utm { zone: 47, north: true }.wkt().contains("central_meridian\",99"));
    }

    #[test]
    fn hostile_numbers_give_none_not_a_panic() {
        let utm = Crs::Utm { zone: 47, north: true };
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300] {
            assert!(utm.from_lonlat(v, 0.0).is_none() || v.abs() < 1e3);
            assert!(utm.from_lonlat(100.0, v).is_none());
            assert!(utm.to_lonlat(v, 0.0).is_none());
            assert!(utm.to_lonlat(500_000.0, v).is_none());
            assert!(Crs::WebMercator.to_lonlat(v, v).is_none() || v.is_finite());
            assert!(Crs::WebMercator.from_lonlat(v, v).is_none());
        }
        assert!(Crs::Utm { zone: 0, north: true }.from_lonlat(10.0, 10.0).is_none());
        assert!(Crs::Utm { zone: 99, north: true }.to_lonlat(500_000.0, 1.0).is_none());
        assert!(utm.from_lonlat(-120.0, 10.0).is_none(), "far outside the zone");
    }
}
