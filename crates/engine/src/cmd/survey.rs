//! Survey points: IMPORTPOINTS reads a point file (CSV, TXT, space or tab separated) written by a
//! total station, GNSS receiver or survey package and draws each point as a POINT with its number,
//! elevation and description as text, on their own layers.
//!
//! The column order is a string of letters, one per column: `P` point number, `N` northing,
//! `E` easting, `Z` elevation, `D` description (`Y` and `X` are northing and easting). The default,
//! `PNEZD`, is the most common export order. A point is drawn at (E, N, Z).

use cadcraft_doc::{Common, EntityKind, HAlign, Point, Text, VAlign};
use cadcraft_geom::Vec3;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most points imported by one command (hostile or mistaken files).
const MAX_POINTS: usize = 500_000;
/// Largest point file read, in bytes.
const MAX_FILE: usize = 64 * 1024 * 1024;
/// Longest point number or description kept, in characters.
const MAX_LABEL: usize = 256;
/// Largest coordinate magnitude accepted (a typo or a unit mix-up shows up far beyond this).
const MAX_COORD: f64 = 1.0e12;
/// How many skipped line numbers are reported back.
const MAX_REPORTED_SKIPS: usize = 10;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("importpoints", "Import Survey Points", run_import).alias(&["ptimport", "importsurvey"]).params(
        "{path | text, format?: \"PNEZD\" (letters P N E Z D, X, Y), textHeight?: 1, decimals?: 2, \
                 layerPrefix?: \"SURVEY\", showNumber?: true, showElevation?: true, showDescription?: true, symbol?: true}",
    )]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Col {
    Number,
    North,
    East,
    Elev,
    Desc,
}

/// One surveyed point.
#[derive(Clone, Debug, PartialEq)]
struct SurveyPoint {
    number: String,
    east: f64,
    north: f64,
    elev: Option<f64>,
    desc: String,
}

#[derive(Default, Debug)]
struct Parsed {
    points: Vec<SurveyPoint>,
    /// 1-based numbers of the lines that held data but could not be read (a header line is one).
    skipped: Vec<usize>,
    /// The file held more than `MAX_POINTS` points.
    truncated: bool,
}

fn parse_format(spec: &str) -> std::result::Result<Vec<Col>, String> {
    let mut cols = Vec::new();
    for ch in spec.chars().filter(|c| !c.is_whitespace() && *c != ',') {
        let c = match ch.to_ascii_uppercase() {
            'P' => Col::Number,
            'N' | 'Y' => Col::North,
            'E' | 'X' => Col::East,
            'Z' => Col::Elev,
            'D' => Col::Desc,
            other => return Err(format!("`format` has an unknown column letter `{other}` (use P, N, E, Z, D)")),
        };
        if cols.contains(&c) {
            return Err("`format` names a column twice".into());
        }
        cols.push(c);
    }
    if !cols.contains(&Col::North) || !cols.contains(&Col::East) {
        return Err("`format` needs a northing (N or Y) and an easting (E or X) column".into());
    }
    Ok(cols)
}

/// The separator of a point file, from its first data line: comma, tab, semicolon, else white space.
fn detect_delimiter(text: &str) -> Option<char> {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty() && !is_comment(l))?;
    [',', '\t', ';'].into_iter().find(|d| first.contains(*d))
}

fn is_comment(line: &str) -> bool {
    line.starts_with('#') || line.starts_with("//")
}

/// Split `line` into at most `n` fields; the last field keeps the rest of the line (descriptions
/// may hold spaces or the separator).
fn split_fields(line: &str, n: usize, delim: Option<char>) -> Vec<&str> {
    match delim {
        Some(d) => line.splitn(n, d).map(str::trim).collect(),
        None => {
            let mut out = Vec::new();
            let mut rest = line.trim_start();
            while out.len() + 1 < n && !rest.is_empty() {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                out.push(rest.get(..end).unwrap_or(""));
                rest = rest.get(end..).unwrap_or("").trim_start();
            }
            if !rest.is_empty() {
                out.push(rest.trim_end());
            }
            out
        }
    }
}

fn number(s: &str) -> Option<f64> {
    let v: f64 = s.trim().parse().ok()?;
    (v.is_finite() && v.abs() <= MAX_COORD).then_some(v)
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_LABEL).collect::<String>().trim().to_string()
}

fn parse_points(text: &str, cols: &[Col]) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delim = detect_delimiter(text);
    let mut out = Parsed::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || is_comment(line) {
            continue;
        }
        let fields = split_fields(line, cols.len(), delim);
        let get = |c: Col| cols.iter().position(|x| *x == c).and_then(|k| fields.get(k).copied());
        let (Some(east), Some(north)) = (get(Col::East).and_then(number), get(Col::North).and_then(number)) else {
            out.skipped.push(i + 1);
            continue;
        };
        if out.points.len() >= MAX_POINTS {
            out.truncated = true;
            break;
        }
        let number_text = get(Col::Number).map(clean).unwrap_or_default();
        out.points.push(SurveyPoint {
            // A file without a number column numbers its points in order.
            number: if number_text.is_empty() && !cols.contains(&Col::Number) { (out.points.len() + 1).to_string() } else { number_text },
            east,
            north,
            elev: get(Col::Elev).and_then(number),
            desc: get(Col::Desc).map(clean).unwrap_or_default(),
        });
    }
    out
}

fn layer_prefix(p: &Value) -> Result<String> {
    let raw = str_param(p, "layerPrefix").unwrap_or("SURVEY").trim();
    if raw.is_empty() || raw.chars().count() > 64 || raw.chars().any(|c| c.is_control() || "<>/\\\":;?*|=`".contains(c)) {
        return Err(bad("importpoints", "`layerPrefix` must be 1 to 64 characters without < > / \\ \" : ; ? * | = `"));
    }
    Ok(raw.to_string())
}

fn read_text(p: &Value) -> Result<String> {
    if let Some(t) = str_param(p, "text") {
        if t.len() > MAX_FILE {
            return Err(bad("importpoints", "`text` is too large"));
        }
        return Ok(t.to_string());
    }
    let Some(_path) = str_param(p, "path") else {
        return Err(bad("importpoints", "`path` (a point file) or `text` is required"));
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = std::fs::read(_path).map_err(|e| bad("importpoints", format!("{_path}: {e}")))?;
        if bytes.len() > MAX_FILE {
            return Err(bad("importpoints", format!("{_path}: file is larger than {} MB", MAX_FILE / (1024 * 1024))));
        }
        // Survey files are often not UTF-8 (a field book's own code page); keep the numbers.
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    #[cfg(target_arch = "wasm32")]
    Err(bad("importpoints", "paths are not available on the web; pass `text`"))
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let cols = parse_format(str_param(p, "format").unwrap_or("PNEZD")).map_err(|m| bad("importpoints", m))?;
    let height = size_param("importpoints", p, "textHeight", false)?.unwrap_or(1.0);
    let decimals = match p.get("decimals") {
        None | Some(Value::Null) => 2,
        Some(v) => {
            v.as_u64().filter(|d| *d <= 8).and_then(|d| usize::try_from(d).ok()).ok_or_else(|| bad("importpoints", "`decimals` must be 0 to 8"))?
        }
    };
    let prefix = layer_prefix(p)?;
    let (show_no, show_elev, show_desc) = (bool_or(p, "showNumber", true), bool_or(p, "showElevation", true), bool_or(p, "showDescription", true));

    let parsed = parse_points(&read_text(p)?, &cols);
    if parsed.points.is_empty() {
        let hint =
            if parsed.skipped.is_empty() { String::new() } else { format!(" ({} lines could not be read: check `format`)", parsed.skipped.len()) };
        return Err(bad("importpoints", format!("no points found{hint}")));
    }

    let layers = [format!("{prefix}-PTS"), format!("{prefix}-NO"), format!("{prefix}-ELEV"), format!("{prefix}-DESC")];
    let space = s.space();
    let base = s.current_common();
    let style = s.doc()?.header.str("TEXTSTYLE", "Standard");
    {
        let d = s.doc_mut()?;
        for l in &layers {
            d.ensure_layer(l);
        }
        // A drawing still on the default point style (a single dot) shows nothing worth picking:
        // use an X sized to the text so the survey points can be seen.
        if bool_or(p, "symbol", true) && d.header.i64("PDMODE", 0) == 0 {
            d.header.set_i64("PDMODE", 3);
            d.header.set_f64("PDSIZE", height * 0.6);
        }
    }

    let on = |layer: &str| Common { layer: layer.to_string(), ..base.clone() };
    let gap = height * 0.6;
    let text = |at: Vec3, value: String| {
        EntityKind::Text(Text {
            insert: at,
            align_pt: None,
            height,
            value,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: style.clone(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        })
    };

    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut texts = 0usize;
    for sp in &parsed.points {
        let z = sp.elev.unwrap_or(0.0);
        let at = Vec3::new(sp.east, sp.north, z);
        min = [min[0].min(sp.east), min[1].min(sp.north)];
        max = [max[0].max(sp.east), max[1].max(sp.north)];
        let d = s.doc_mut()?;
        d.add(&space, on(&layers[0]), EntityKind::Point(Point { p: at, angle: 0.0 }))?;
        // Number above, elevation beside the point, description below, all left aligned.
        let x = sp.east + gap;
        let mut put = |layer: &str, y: f64, value: String| -> Result<()> {
            d.add(&space, on(layer), text(Vec3::new(x, y, z), value))?;
            texts += 1;
            Ok(())
        };
        if show_no && !sp.number.is_empty() {
            put(&layers[1], sp.north + gap, sp.number.clone())?;
        }
        if show_elev && let Some(e) = sp.elev {
            put(&layers[2], sp.north - height / 2.0, format!("{e:.decimals$}"))?;
        }
        if show_desc && !sp.desc.is_empty() {
            put(&layers[3], sp.north - height * 1.7, sp.desc.clone())?;
        }
    }

    let n = parsed.points.len();
    let mut msg = format!("{n} survey points imported");
    if !parsed.skipped.is_empty() {
        msg.push_str(&format!(", {} lines skipped", parsed.skipped.len()));
    }
    if parsed.truncated {
        msg.push_str(&format!(" (stopped at {MAX_POINTS} points)"));
    }
    s.echo(msg.clone());
    Ok(json!({
        "imported": n,
        "labels": texts,
        "skipped": parsed.skipped.len(),
        "skippedLines": parsed.skipped.iter().take(MAX_REPORTED_SKIPS).collect::<Vec<_>>(),
        "truncated": parsed.truncated,
        "layers": layers,
        "bounds": { "min": min, "max": max },
        "message": msg,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(s: &str) -> Vec<Col> {
        parse_format(s).unwrap_or_default()
    }

    #[test]
    fn comma_file_with_header_and_description() {
        let t = "Point,Northing,Easting,Elev,Desc\n1,1000.5,2000.25,10.5,CORNER POST\n2,1010,2005,,TREE\n";
        let p = parse_points(t, &cols("PNEZD"));
        assert_eq!(p.skipped, vec![1], "the header line is skipped");
        assert_eq!(p.points.len(), 2);
        assert_eq!(p.points[0], SurveyPoint { number: "1".into(), east: 2000.25, north: 1000.5, elev: Some(10.5), desc: "CORNER POST".into() });
        assert_eq!(p.points[1].elev, None, "a blank elevation is no elevation");
    }

    #[test]
    fn space_separated_description_keeps_its_spaces() {
        let p = parse_points("7 100 200 5.5 BENCH MARK 3\n", &cols("PNEZD"));
        assert_eq!(p.points.first().map(|q| q.desc.as_str()), Some("BENCH MARK 3"));
    }

    #[test]
    fn xyz_without_numbers_numbers_in_order() {
        let p = parse_points("500 600 7\n510\t610\t8\n", &cols("EN Z"));
        assert_eq!(p.points.iter().map(|q| q.number.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(p.points[1].east, 510.0);
    }

    #[test]
    fn hostile_values_are_skipped() {
        let t = "1,NaN,5,1,A\n2,inf,5,1,B\n3,1e300,5,1,C\n4,10,20,1,D\n";
        let p = parse_points(t, &cols("PNEZD"));
        assert_eq!(p.points.len(), 1);
        assert_eq!(p.skipped.len(), 3);
    }

    #[test]
    fn bad_formats_are_refused() {
        assert!(parse_format("PNZ").is_err(), "no easting");
        assert!(parse_format("PNENZ").is_err(), "a column twice");
        assert!(parse_format("PNEQ").is_err(), "unknown letter");
    }
}
