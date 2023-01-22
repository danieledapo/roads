use serde::{Deserialize, Serialize};

pub mod simplify;
pub mod util;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NominatimEntry {
    pub place_id: i64,
    pub osm_type: String,
    pub osm_id: i64,
    pub display_name: String,
    pub importance: f64,
    pub boundingbox: [String; 4],
    pub r#type: String,
}

#[derive(Serialize, Deserialize)]
struct OverpassForm {
    data: String,
}

#[derive(Serialize, Deserialize)]
struct OverpassResponse {
    elements: Vec<OverpassElement>,
}

#[derive(Serialize, Deserialize)]
struct OverpassElement {
    id: i64,
    geometry: Vec<LatLon>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct LatLon {
    lat: f64,
    lon: f64,
}

pub async fn search(place: &str) -> reqwest::Result<Vec<NominatimEntry>> {
    reqwest::Client::new()
        .get(format!(
            "https://nominatim.openstreetmap.org/search/{place}?format=json"
        ))
        .header(
            reqwest::header::USER_AGENT,
            format!("roads/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await?
        .json()
        .await
}

pub async fn fetch_roads(entry: &NominatimEntry) -> reqwest::Result<Vec<Vec<(f64, f64)>>> {
    let query = if entry.osm_type != "relation" && entry.osm_type != "way" {
        format!(
            r#"[out:json][timeout:60][bbox:{},{},{},{}];
// way[highway~"^(motorway|primary|secondary|tertiary)|residential"];
way[highway];
out geom;"#,
            entry.boundingbox[0], entry.boundingbox[2], entry.boundingbox[1], entry.boundingbox[3],
        )
    } else {
        format!(
            r#"[out:json][timeout:60];
area({})->.a;
// way(area.a)[highway~"^(motorway|primary|secondary|tertiary)|residential"];
way(area.a)[highway];
out geom;"#,
            if entry.osm_type == "relation" {
                3_600_000_000 + entry.osm_id
            } else if entry.osm_type == "way" {
                2_400_000_000 + entry.osm_id
            } else {
                unreachable!()
            },
        )
    };

    let client = reqwest::Client::new();
    let r: OverpassResponse = client
        .post("https://overpass-api.de/api/interpreter")
        .form(&OverpassForm { data: query })
        .header(reqwest::header::CONTENT_TYPE, "application/osm3s+xml")
        .send()
        .await?
        .json()
        .await?;

    Ok(r.elements
        .into_iter()
        .map(|e| e.geometry.into_iter().map(|p| p.to_xy()).collect())
        .collect())
}

impl LatLon {
    /// Earth radius in meters.
    const EARTH_RADIUS: f64 = 6378137.0;

    pub fn to_xy(self) -> (f64, f64) {
        // https://wiki.openstreetmap.org/wiki/Mercator

        use std::f64::consts::FRAC_PI_4;

        let x = self.lon.to_radians() * Self::EARTH_RADIUS;
        let y = f64::ln(f64::tan(self.lat.to_radians() / 2.0 + FRAC_PI_4)) * Self::EARTH_RADIUS;

        (x, y)
    }
}
