//! Charger Pack builder: normalizes the three national open Charger feeds
//! (NL DOT-NL OCPI JSON, BE transportdata.be OCPI JSON, LU Chargy KML) to an
//! OCPI-like record -- "connectors, max_electric_power, operator, access"
//! per ADR 0005 point 1 -- and writes the `.json` Charger Pack artifact.
//! Ports the throwaway `prototype/vertical-slice`'s three parsers, but
//! keeps every connector of an included location (not just the qualifying
//! CCS one) so the app can show connector types on a Charging Stop.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// One connector at a charger location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub standard: String,
    pub power_kw: f32,
}

/// One charger location, normalized to an OCPI-like record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChargerRecord {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub operator: Option<String>,
    pub access: Option<String>,
    pub country: String,
    pub max_power_kw: f32,
    pub connectors: Vec<Connector>,
    pub source: String,
}

/// A connector counts as CCS (DC) for the inclusion rule if its `standard`
/// contains "COMBO" (case-insensitive) -- covers OCPI's `IEC_62196_T2_COMBO`.
fn is_ccs(standard: &str) -> bool {
    standard.to_uppercase().contains("COMBO")
}

const MIN_DC_POWER_W: f64 = 50_000.0;

// ---------------------------------------------------------------------
// OCPI 2.2.1 Locations (NL/BE)
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct OcpiLocation {
    id: Option<String>,
    name: Option<String>,
    country_code: Option<String>,
    coordinates: Option<OcpiCoords>,
    #[serde(default)]
    evses: Vec<OcpiEvse>,
    operator: Option<OcpiBusinessDetails>,
    access: Option<String>,
}

#[derive(Deserialize)]
struct OcpiBusinessDetails {
    name: Option<String>,
}

#[derive(Deserialize)]
struct OcpiCoords {
    latitude: String,
    longitude: String,
}

#[derive(Deserialize)]
struct OcpiEvse {
    #[serde(default)]
    connectors: Vec<OcpiConnector>,
}

#[derive(Deserialize)]
struct OcpiConnector {
    standard: Option<String>,
    max_electric_power: Option<f64>,
    max_voltage: Option<f64>,
    max_amperage: Option<f64>,
}

/// Connector power in Watts: `max_electric_power` when present and
/// positive, else `max_voltage * max_amperage`.
fn connector_power_w(c: &OcpiConnector) -> f64 {
    c.max_electric_power
        .filter(|p| *p > 0.0)
        .unwrap_or_else(|| c.max_voltage.unwrap_or(0.0) * c.max_amperage.unwrap_or(0.0))
}

/// `None` if the location has no coordinates or no qualifying CCS
/// connector (>=50 kW); otherwise a record carrying every connector at the
/// location, with `max_power_kw` set to the best qualifying CCS power.
fn ocpi_location_to_charger(
    loc: &OcpiLocation,
    index: usize,
    source: &str,
    default_country: &str,
) -> Option<ChargerRecord> {
    let coords = loc.coordinates.as_ref()?;
    let lat: f64 = coords.latitude.parse().ok()?;
    let lon: f64 = coords.longitude.parse().ok()?;

    let mut connectors = Vec::new();
    let mut best_ccs_power_w = 0.0f64;
    for evse in &loc.evses {
        for c in &evse.connectors {
            let standard = c.standard.clone().unwrap_or_default();
            let power_w = connector_power_w(c);
            if is_ccs(&standard) && power_w >= MIN_DC_POWER_W {
                best_ccs_power_w = best_ccs_power_w.max(power_w);
            }
            connectors.push(Connector {
                standard,
                power_kw: (power_w / 1000.0) as f32,
            });
        }
    }
    if best_ccs_power_w <= 0.0 {
        return None;
    }

    let native_id = loc.id.clone().unwrap_or_else(|| index.to_string());
    Some(ChargerRecord {
        id: format!("{source}:{native_id}"),
        name: loc.name.clone().unwrap_or_default(),
        lat,
        lon,
        operator: loc.operator.as_ref().and_then(|o| o.name.clone()),
        access: loc.access.clone(),
        country: loc
            .country_code
            .clone()
            .unwrap_or_else(|| default_country.to_string()),
        max_power_kw: (best_ccs_power_w / 1000.0) as f32,
        connectors,
        source: source.to_string(),
    })
}

/// Parses an OCPI Locations JSON document, accepting either a bare array
/// (BE transportdata.be) or a `{"data": [...]}` wrapper (NL DOT-NL).
fn parse_ocpi_str(s: &str, source: &str, default_country: &str) -> Vec<ChargerRecord> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(s) else {
        return Vec::new();
    };
    let locations: Vec<OcpiLocation> = if let Some(arr) = value.as_array() {
        arr.iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
    } else if let Some(arr) = value.get("data").and_then(|d| d.as_array()) {
        arr.iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
    } else {
        Vec::new()
    };
    locations
        .iter()
        .enumerate()
        .filter_map(|(i, l)| ocpi_location_to_charger(l, i, source, default_country))
        .collect()
}

/// Reads the NDW feed (`ndw_chargers.json.gz`, gzip-compressed OCPI JSON).
pub fn parse_ndw_gz(path: &Path) -> io::Result<Vec<ChargerRecord>> {
    let file = fs::File::open(path)?;
    let mut s = String::new();
    GzDecoder::new(file).read_to_string(&mut s)?;
    Ok(parse_ocpi_str(&s, "ndw", "NL"))
}

/// Reads the transportdata.be feed (`road_chargers.json`, plain OCPI JSON).
pub fn parse_roadbe(path: &Path) -> io::Result<Vec<ChargerRecord>> {
    let s = fs::read_to_string(path)?;
    Ok(parse_ocpi_str(&s, "roadbe", "BE"))
}

// ---------------------------------------------------------------------
// Chargy KML (Luxembourg)
// ---------------------------------------------------------------------

/// Only "SuperChargy" placemarks are DC (>=50 kW, CCS assumed, fixed
/// 160 kW); regular Chargy (22 kW AC) placemarks are dropped.
fn parse_chargy_kml_str(s: &str) -> Vec<ChargerRecord> {
    let mut out = Vec::new();
    for chunk in s.split("<Placemark>").skip(1) {
        let end = chunk.find("</Placemark>").unwrap_or(chunk.len());
        let chunk = &chunk[..end];
        let Some(name) = extract_tag(chunk, "name") else {
            continue;
        };
        if !name.contains("SuperChargy") {
            continue;
        }
        let Some(coord_str) = extract_tag(chunk, "coordinates") else {
            continue;
        };
        let parts: Vec<&str> = coord_str.trim().split(',').collect();
        if parts.len() < 2 {
            continue;
        }
        let (Ok(lon), Ok(lat)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) else {
            continue;
        };
        let index = out.len();
        out.push(ChargerRecord {
            id: format!("chargy:{index}"),
            name,
            lat,
            lon,
            operator: None,
            access: None,
            country: "LU".to_string(),
            max_power_kw: 160.0,
            connectors: vec![Connector {
                standard: "COMBO".to_string(),
                power_kw: 160.0,
            }],
            source: "chargy".to_string(),
        });
    }
    out
}

fn extract_tag(chunk: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = chunk.find(&open)? + open.len();
    let end = chunk[start..].find(&close)? + start;
    Some(chunk[start..end].to_string())
}

/// Reads the Chargy feed (`chargy.kml`).
pub fn parse_chargy_kml(path: &Path) -> io::Result<Vec<ChargerRecord>> {
    let s = fs::read_to_string(path)?;
    Ok(parse_chargy_kml_str(&s))
}

// ---------------------------------------------------------------------
// bbox filter + Charger Pack writer
// ---------------------------------------------------------------------

/// Keeps only records inside `[min_lat, max_lat] x [min_lon, max_lon]`.
/// The orchestrator clips to the graph's bbox plus a 0.2 deg margin so
/// chargers just outside a region's roads (border extract clipping) still
/// show up.
pub fn filter_bbox(
    records: Vec<ChargerRecord>,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
) -> Vec<ChargerRecord> {
    records
        .into_iter()
        .filter(|r| r.lat >= min_lat && r.lat <= max_lat && r.lon >= min_lon && r.lon <= max_lon)
        .collect()
}

/// Writes the Charger Pack artifact: one JSON file,
/// `{"format": "cpack-1", "region_id", "built_at_epoch", "charger_count", "chargers"}`.
pub fn write_charger_pack(
    path: &Path,
    region_id: &str,
    records: &[ChargerRecord],
) -> io::Result<()> {
    let built_at_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    let doc = json!({
        "format": "cpack-1",
        "region_id": region_id,
        "built_at_epoch": built_at_epoch,
        "charger_count": records.len(),
        "chargers": records,
    });
    fs::write(path, serde_json::to_vec(&doc)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BE_BARE_ARRAY: &str = r#"[
        {
            "id": "BE*ROAD*001",
            "name": "Antwerp DC Hub",
            "country_code": "BE",
            "coordinates": {"latitude": "51.2", "longitude": "4.4"},
            "operator": {"name": "Road NV"},
            "access": "public",
            "evses": [
                {"connectors": [
                    {"standard": "IEC_62196_T2_COMBO", "max_electric_power": 150000},
                    {"standard": "IEC_62196_T2", "max_electric_power": 22000}
                ]}
            ]
        },
        {
            "name": "AC-only site",
            "coordinates": {"latitude": "51.3", "longitude": "4.5"},
            "evses": [
                {"connectors": [{"standard": "IEC_62196_T2", "max_electric_power": 22000}]}
            ]
        }
    ]"#;

    const NL_DATA_WRAPPER: &str = r#"{
        "status_code": 1000,
        "data": [
            {
                "id": "NL*NDW*042",
                "name": "Utrecht Fastcharge",
                "coordinates": {"latitude": "52.1", "longitude": "5.1"},
                "evses": [
                    {"connectors": [
                        {"standard": "IEC_62196_T2_COMBO", "max_voltage": 500, "max_amperage": 125}
                    ]}
                ]
            }
        ]
    }"#;

    const KML_SNIPPET: &str = r#"<kml><Document>
        <Placemark><name>SuperChargy Diekirch</name><Point><coordinates>6.1667,49.8667,0</coordinates></Point></Placemark>
        <Placemark><name>Chargy Ettelbruck</name><Point><coordinates>6.1,49.85,0</coordinates></Point></Placemark>
    </Document></kml>"#;

    #[test]
    fn ocpi_bare_array_includes_only_locations_with_qualifying_ccs() {
        let out = parse_ocpi_str(BE_BARE_ARRAY, "roadbe", "BE");
        assert_eq!(out.len(), 1);
        let r = &out[0];
        assert_eq!(r.id, "roadbe:BE*ROAD*001");
        assert_eq!(r.source, "roadbe");
        assert_eq!(r.country, "BE");
        assert_eq!(r.operator.as_deref(), Some("Road NV"));
        assert_eq!(r.access.as_deref(), Some("public"));
        assert_eq!(r.max_power_kw, 150.0);
        // Both connectors are retained, not just the qualifying CCS one.
        assert_eq!(r.connectors.len(), 2);
    }

    #[test]
    fn ocpi_data_wrapper_falls_back_to_running_index_and_volts_times_amps() {
        let out = parse_ocpi_str(NL_DATA_WRAPPER, "ndw", "NL");
        assert_eq!(out.len(), 1);
        let r = &out[0];
        assert_eq!(r.id, "ndw:NL*NDW*042");
        // 500V * 125A = 62_500 W = 62.5 kW.
        assert_eq!(r.max_power_kw, 62.5);
        assert_eq!(r.connectors[0].power_kw, 62.5);
    }

    #[test]
    fn ocpi_missing_native_id_falls_back_to_running_index() {
        const NO_ID: &str = r#"[{
            "name": "No native id",
            "coordinates": {"latitude": "50.0", "longitude": "5.0"},
            "evses": [{"connectors": [{"standard": "COMBO", "max_electric_power": 50000}]}]
        }]"#;
        let out = parse_ocpi_str(NO_ID, "roadbe", "BE");
        assert_eq!(out[0].id, "roadbe:0");
    }

    #[test]
    fn chargy_kml_keeps_only_superchargy_placemarks_at_fixed_160kw() {
        let out = parse_chargy_kml_str(KML_SNIPPET);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "SuperChargy Diekirch");
        assert_eq!(out[0].country, "LU");
        assert_eq!(out[0].max_power_kw, 160.0);
        assert_eq!(
            out[0].connectors,
            vec![Connector {
                standard: "COMBO".into(),
                power_kw: 160.0
            }]
        );
        assert!((out[0].lat - 49.8667).abs() < 1e-6);
        assert!((out[0].lon - 6.1667).abs() < 1e-6);
    }

    #[test]
    fn bbox_filter_keeps_only_records_inside_the_box() {
        let records = vec![
            ChargerRecord {
                id: "a".into(),
                name: "in".into(),
                lat: 50.0,
                lon: 6.0,
                operator: None,
                access: None,
                country: "LU".into(),
                max_power_kw: 50.0,
                connectors: vec![],
                source: "chargy".into(),
            },
            ChargerRecord {
                id: "b".into(),
                name: "out".into(),
                lat: 60.0,
                lon: 6.0,
                operator: None,
                access: None,
                country: "LU".into(),
                max_power_kw: 50.0,
                connectors: vec![],
                source: "chargy".into(),
            },
        ];
        let kept = filter_bbox(records, 49.0, 5.0, 51.0, 7.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "a");
    }

    #[test]
    fn write_charger_pack_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lu-dev-chargers.json");
        let records = parse_chargy_kml_str(KML_SNIPPET);
        write_charger_pack(&path, "lu-dev", &records).unwrap();

        let bytes = fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["format"], "cpack-1");
        assert_eq!(value["region_id"], "lu-dev");
        assert_eq!(value["charger_count"], 1);
        assert_eq!(value["chargers"].as_array().unwrap().len(), 1);
        assert!(value["built_at_epoch"].as_u64().unwrap() > 0);
    }
}
