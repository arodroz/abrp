//! Copernicus GLO-30 elevation sampling for Region Packs, per ADR 0005 §4
//! (wayfinder #35): samples every geometry vertex from the public GLO-30
//! COGs on S3, smooths each edge's profile with a ~150 m rolling median, and
//! writes `GeomVertex.elev_m` plus per-edge `EdgeHot.ascent_m`/`descent_m`.
//!
//! Decoded tiles are held as `Vec<f32>` in memory for the run's lifetime.
//! GLO-30 tiles are 1°×1°, ~3600×3600 (or narrower above 50°N -- see
//! `Tile::sample`); a Region Pack corridor needs on the order of 30 tiles,
//! roughly 1.5 GB resident. Acceptable on the build Mac, not for a phone.
//!
//! The network/cache/decode path (`TileStore::load`) is exercised only by
//! `apply_elevation` itself; sampling, smoothing and aggregation are plain
//! functions over an in-memory `TileStore` so tests can pre-populate tiles
//! and run fully offline (see `#[cfg(test)] TileStore::from_tiles`).

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use packs::{GeomVertex, RegionGraphModel};

/// Half-width of the rolling median smoothing window (ADR 0005 §4: "100-200
/// m median"). A vertex's smoothed value is the median of every sample
/// within this distance of it along the edge's chain.
const SMOOTH_HALF_WINDOW_M: f64 = 75.0;

/// GLO-30 carries no explicit nodata sentinel over land; values further from
/// sea level than this are treated as decode/edge-case garbage and clamped
/// to 0 m defensively.
const ABSURD_ELEV_M: f32 = 9000.0;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug)]
pub enum ElevationError {
    Http(Box<ureq::Error>),
    Tiff(tiff::TiffError),
    Io(std::io::Error),
    UnsupportedFormat(&'static str),
}

impl fmt::Display for ElevationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ElevationError::Http(e) => write!(f, "dem tile download failed: {e}"),
            ElevationError::Tiff(e) => write!(f, "dem tile decode failed: {e}"),
            ElevationError::Io(e) => write!(f, "io error: {e}"),
            ElevationError::UnsupportedFormat(msg) => {
                write!(f, "unsupported dem tile format: {msg}")
            }
        }
    }
}

impl std::error::Error for ElevationError {}

impl From<std::io::Error> for ElevationError {
    fn from(e: std::io::Error) -> Self {
        ElevationError::Io(e)
    }
}

/// Stats from one `apply_elevation` run.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElevationStats {
    pub tiles_fetched: usize,
    pub tiles_cache_hits: usize,
    pub tiles_missing: usize,
    pub vertices_sampled: usize,
    pub min_elev_m: i16,
    pub max_elev_m: i16,
    pub edges_updated: usize,
}

/// Builds the GLO-30 tile identifier for the tile whose SW corner is
/// `(lat_floor, lon_floor)`, e.g. `(49, 6)` -> `Copernicus_DSM_COG_10_N49_00_E006_00_DEM`.
fn tile_id(lat_floor: i32, lon_floor: i32) -> String {
    let (ns, lat_abs) = if lat_floor >= 0 {
        ('N', lat_floor)
    } else {
        ('S', -lat_floor)
    };
    let (ew, lon_abs) = if lon_floor >= 0 {
        ('E', lon_floor)
    } else {
        ('W', -lon_floor)
    };
    format!("Copernicus_DSM_COG_10_{ns}{lat_abs:02}_00_{ew}{lon_abs:03}_00_DEM")
}

fn tile_url(lat_floor: i32, lon_floor: i32) -> String {
    let id = tile_id(lat_floor, lon_floor);
    format!("https://copernicus-dem-30m.s3.amazonaws.com/{id}/{id}.tif")
}

/// One decoded DEM tile: `width`x`height` grid of metres above sea level,
/// row-major with row 0 the tile's north edge and column 0 its west edge.
struct Tile {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Tile {
    /// Bilinear-samples elevation at `(lat, lon)`, which must fall within
    /// this tile's `[lat_floor, lat_floor+1) x [lon_floor, lon_floor+1)`
    /// cell. GLO-30's column count varies by latitude band (3600 below
    /// 50°N, 2400 for 50-60°N), so pixel spacing is derived from `width`/
    /// `height` here, never hardcoded. Pixel-center aligned: pixel `(0,0)`
    /// sits at half a pixel south-east of the tile's NW corner, so this
    /// carries a systematic sub-pixel (< 15 m) offset that's acceptable at
    /// 30 m resolution.
    fn sample(&self, lat_floor: i32, lon_floor: i32, lat: f64, lon: f64) -> f32 {
        let row_f = self.height as f64 * ((lat_floor + 1) as f64 - lat) - 0.5;
        let col_f = self.width as f64 * (lon - lon_floor as f64) - 0.5;
        bilinear(&self.data, self.width, self.height, row_f, col_f)
    }
}

/// Bilinear interpolation over a `width`x`height` row-major grid at
/// fractional coordinate `(row_f, col_f)`, where integer coordinates land
/// exactly on grid samples. Out-of-range coordinates (including exactly on
/// the last row/column) clamp to the grid's edge.
fn bilinear(data: &[f32], width: usize, height: usize, row_f: f64, col_f: f64) -> f32 {
    let row_f = row_f.clamp(0.0, (height - 1) as f64);
    let col_f = col_f.clamp(0.0, (width - 1) as f64);
    let r0 = row_f.floor() as usize;
    let c0 = col_f.floor() as usize;
    let r1 = (r0 + 1).min(height - 1);
    let c1 = (c0 + 1).min(width - 1);
    let fr = row_f - r0 as f64;
    let fc = col_f - c0 as f64;

    let v00 = data[r0 * width + c0] as f64;
    let v01 = data[r0 * width + c1] as f64;
    let v10 = data[r1 * width + c0] as f64;
    let v11 = data[r1 * width + c1] as f64;

    let top = v00 + (v01 - v00) * fc;
    let bottom = v10 + (v11 - v10) * fc;
    (top + (bottom - top) * fr) as f32
}

/// Decodes a GLO-30 COG (DEFLATE-compressed tiled float32) from `path`.
fn decode_tile(path: &Path) -> Result<Tile, ElevationError> {
    let file = fs::File::open(path)?;
    let mut decoder = tiff::decoder::Decoder::new(file).map_err(ElevationError::Tiff)?;
    let (width, height) = decoder.dimensions().map_err(ElevationError::Tiff)?;
    let image = decoder.read_image().map_err(ElevationError::Tiff)?;
    let data = match image {
        tiff::decoder::DecodingResult::F32(v) => v,
        _ => {
            return Err(ElevationError::UnsupportedFormat(
                "expected a single-band f32 DEM tile",
            ))
        }
    };
    Ok(Tile {
        width: width as usize,
        height: height as usize,
        data,
    })
}

/// A cache of decoded DEM tiles keyed by `(lat_floor, lon_floor)` (the
/// tile's SW corner). `None` entries are cached ocean misses. Production
/// code populates this lazily via `sample` -> `load`; tests bypass `load`
/// entirely by pre-populating `tiles` through `from_tiles`.
struct TileStore {
    tiles: HashMap<(i32, i32), Option<Tile>>,
    cache_dir: PathBuf,
    tiles_fetched: usize,
    tiles_cache_hits: usize,
    tiles_missing: usize,
}

impl TileStore {
    fn new(cache_dir: PathBuf) -> Self {
        TileStore {
            tiles: HashMap::new(),
            cache_dir,
            tiles_fetched: 0,
            tiles_cache_hits: 0,
            tiles_missing: 0,
        }
    }

    #[cfg(test)]
    fn from_tiles(tiles: HashMap<(i32, i32), Option<Tile>>) -> Self {
        TileStore {
            tiles,
            cache_dir: PathBuf::new(),
            tiles_fetched: 0,
            tiles_cache_hits: 0,
            tiles_missing: 0,
        }
    }

    /// Samples elevation at `(lat, lon)` in metres, loading (and caching)
    /// the covering tile on first use. Ocean tiles sample as 0 m. Absurd
    /// values are clamped to 0 m defensively (see `ABSURD_ELEV_M`).
    fn sample(&mut self, lat: f64, lon: f64) -> Result<f32, ElevationError> {
        let key = (lat.floor() as i32, lon.floor() as i32);
        if !self.tiles.contains_key(&key) {
            self.load(key.0, key.1)?;
        }
        let raw = match self.tiles.get(&key).expect("just inserted above") {
            Some(tile) => tile.sample(key.0, key.1, lat, lon),
            None => 0.0,
        };
        Ok(if raw.abs() > ABSURD_ELEV_M { 0.0 } else { raw })
    }

    /// Loads tile `(lat_floor, lon_floor)`: from the on-disk `.missing`
    /// marker or cached `.tif` if present, otherwise fetches it from S3 --
    /// caching a 404 as an empty `.missing` marker so reruns don't
    /// re-request it. Any other HTTP error is propagated.
    fn load(&mut self, lat_floor: i32, lon_floor: i32) -> Result<(), ElevationError> {
        let id = tile_id(lat_floor, lon_floor);
        let tif_path = self.cache_dir.join(format!("{id}.tif"));
        let missing_path = self.cache_dir.join(format!("{id}.missing"));

        if missing_path.exists() {
            self.tiles_missing += 1;
            self.tiles.insert((lat_floor, lon_floor), None);
            return Ok(());
        }
        if tif_path.exists() {
            let tile = decode_tile(&tif_path)?;
            self.tiles_cache_hits += 1;
            self.tiles.insert((lat_floor, lon_floor), Some(tile));
            return Ok(());
        }

        match ureq::get(&tile_url(lat_floor, lon_floor)).call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                response.into_reader().read_to_end(&mut bytes)?;
                fs::write(&tif_path, &bytes)?;
                let tile = decode_tile(&tif_path)?;
                self.tiles_fetched += 1;
                self.tiles.insert((lat_floor, lon_floor), Some(tile));
                Ok(())
            }
            Err(ureq::Error::Status(404, _)) => {
                fs::write(&missing_path, [])?;
                self.tiles_missing += 1;
                self.tiles.insert((lat_floor, lon_floor), None);
                Ok(())
            }
            Err(e) => Err(ElevationError::Http(Box::new(e))),
        }
    }
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (lat1r, lat2r) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + lat1r.cos() * lat2r.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Running distance (metres) of each vertex from the chain's start.
fn cumulative_distances_m(verts: &[GeomVertex]) -> Vec<f64> {
    let mut cumdist = vec![0.0; verts.len()];
    for i in 1..verts.len() {
        cumdist[i] = cumdist[i - 1]
            + haversine_m(
                verts[i - 1].lat as f64,
                verts[i - 1].lon as f64,
                verts[i].lat as f64,
                verts[i].lon as f64,
            );
    }
    cumdist
}

/// Rolling median smoothing over a `±SMOOTH_HALF_WINDOW_M` window along
/// `cumdist`: vertex `i`'s smoothed value is the median of every `raw[j]`
/// with `|cumdist[j] - cumdist[i]| <= SMOOTH_HALF_WINDOW_M`. `cumdist` and
/// `raw` must be the same length and `cumdist` must be non-decreasing (true
/// of any real edge chain, since it's a running sum of non-negative
/// distances).
///
/// The first and last values are pinned to the RAW samples, not smoothed:
/// an edge-end vertex only has a one-sided window, whose median is biased
/// toward the edge's interior -- on a monotone climb that shaves real
/// ascent off both ends, and since smoothing is per-edge the loss compounds
/// along a route instead of telescoping away (measured on the lu-dev pack:
/// a 75 km route with a true +120 m endpoint delta summed to -45 m). Both
/// edges at a junction sample the same raw value, so with pinned endpoints
/// per-edge nets telescope exactly to junction elevation deltas. The cost
/// is that a DEM spike exactly at a junction is not smoothed (it inflates
/// ascent and descent symmetrically on both adjacent edges, no net bias).
fn median_smooth(cumdist: &[f64], raw: &[f32]) -> Vec<f32> {
    let n = raw.len();
    let mut out = Vec::with_capacity(n);
    let mut lo = 0usize;
    let mut hi = 0usize;
    let mut window: Vec<f32> = Vec::new();

    for i in 0..n {
        while cumdist[i] - cumdist[lo] > SMOOTH_HALF_WINDOW_M {
            lo += 1;
        }
        while hi + 1 < n && cumdist[hi + 1] - cumdist[i] <= SMOOTH_HALF_WINDOW_M {
            hi += 1;
        }
        window.clear();
        window.extend_from_slice(&raw[lo..=hi]);
        window.sort_by(|a, b| a.total_cmp(b));
        let mid = window.len() / 2;
        out.push(if window.len() % 2 == 1 {
            window[mid]
        } else {
            (window[mid - 1] + window[mid]) / 2.0
        });
    }
    if n > 0 {
        out[0] = raw[0];
        out[n - 1] = raw[n - 1];
    }
    out
}

fn clamp_to_i16(v: f32) -> i16 {
    v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Samples, smooths and aggregates elevation for every edge in `model`
/// against `store`. Shared by `apply_elevation` (production, network-backed
/// store) and this module's tests (offline, pre-populated store).
fn run(
    model: &mut RegionGraphModel,
    store: &mut TileStore,
) -> Result<ElevationStats, ElevationError> {
    let mut stats = ElevationStats {
        min_elev_m: i16::MAX,
        max_elev_m: i16::MIN,
        ..Default::default()
    };

    let RegionGraphModel {
        edges, geometry, ..
    } = model;

    let mut raw = Vec::new();
    for edge in edges.iter_mut() {
        let start = edge.geom_offset as usize;
        let count = edge.geom_count as usize;
        let verts = &mut geometry[start..start + count];
        if verts.is_empty() {
            continue;
        }

        raw.clear();
        for v in verts.iter() {
            raw.push(store.sample(v.lat as f64, v.lon as f64)?);
        }
        let cumdist = cumulative_distances_m(verts);
        let smoothed = median_smooth(&cumdist, &raw);

        let mut ascent = 0.0_f64;
        let mut descent = 0.0_f64;
        for w in smoothed.windows(2) {
            let delta = (w[1] - w[0]) as f64;
            if delta > 0.0 {
                ascent += delta;
            } else {
                descent -= delta;
            }
        }
        edge.ascent_m = ascent as f32;
        edge.descent_m = descent as f32;

        for (v, &s) in verts.iter_mut().zip(smoothed.iter()) {
            let elev = clamp_to_i16(s);
            v.elev_m = elev;
            stats.min_elev_m = stats.min_elev_m.min(elev);
            stats.max_elev_m = stats.max_elev_m.max(elev);
        }
        stats.vertices_sampled += verts.len();
        stats.edges_updated += 1;
    }

    stats.tiles_fetched = store.tiles_fetched;
    stats.tiles_cache_hits = store.tiles_cache_hits;
    stats.tiles_missing = store.tiles_missing;
    if stats.vertices_sampled == 0 {
        stats.min_elev_m = 0;
        stats.max_elev_m = 0;
    }
    Ok(stats)
}

/// Samples Copernicus GLO-30 at every geometry vertex in `model`, smooths
/// each edge's elevation profile with a rolling median, and writes the
/// smoothed values into `model.geometry[..].elev_m` and the resulting
/// ascent/descent into `model.edges[..]`. Downloaded tiles are cached under
/// `cache_dir` (created if absent); ocean tiles (HTTP 404) are cached as
/// empty `.missing` markers.
pub fn apply_elevation(
    model: &mut RegionGraphModel,
    cache_dir: &Path,
) -> Result<ElevationStats, ElevationError> {
    fs::create_dir_all(cache_dir)?;
    let mut store = TileStore::new(cache_dir.to_path_buf());
    run(model, &mut store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use packs::{EdgeHot, NodeRecord, SnapGridModel, CH_MIDDLE_NODE_NONE};
    use tempfile::tempdir;

    fn geom(lat: f32, lon: f32) -> GeomVertex {
        GeomVertex {
            lat,
            lon,
            elev_m: 0,
            _pad: 0,
        }
    }

    // --- tile id / URL formatting -----------------------------------

    #[test]
    fn tile_id_formats_all_four_hemisphere_quadrants() {
        assert_eq!(tile_id(49, 6), "Copernicus_DSM_COG_10_N49_00_E006_00_DEM");
        assert_eq!(tile_id(50, -1), "Copernicus_DSM_COG_10_N50_00_W001_00_DEM");
        assert_eq!(tile_id(-34, 18), "Copernicus_DSM_COG_10_S34_00_E018_00_DEM");
        assert_eq!(tile_id(-1, -1), "Copernicus_DSM_COG_10_S01_00_W001_00_DEM");
    }

    #[test]
    fn tile_url_wraps_the_id_in_the_s3_path() {
        let url = tile_url(49, 6);
        assert_eq!(
            url,
            "https://copernicus-dem-30m.s3.amazonaws.com/\
             Copernicus_DSM_COG_10_N49_00_E006_00_DEM/\
             Copernicus_DSM_COG_10_N49_00_E006_00_DEM.tif"
        );
    }

    // --- bilinear sampling --------------------------------------------

    /// 4x4 grid where value == row*10 + col: bilinear reconstructs any
    /// point on this plane exactly.
    fn gradient_grid() -> Vec<f32> {
        (0..4)
            .flat_map(|r| (0..4).map(move |c| (r * 10 + c) as f32))
            .collect()
    }

    #[test]
    fn bilinear_is_exact_on_grid_points() {
        let data = gradient_grid();
        assert_eq!(bilinear(&data, 4, 4, 0.0, 0.0), 0.0);
        assert_eq!(bilinear(&data, 4, 4, 3.0, 3.0), 33.0);
        assert_eq!(bilinear(&data, 4, 4, 2.0, 1.0), 21.0);
    }

    #[test]
    fn bilinear_interpolates_interior_points() {
        let data = gradient_grid();
        // Midpoint of the 2x2 cell at rows/cols [1,2]: average of 12,13,22,23.
        assert_eq!(bilinear(&data, 4, 4, 1.5, 2.5), 17.5);
    }

    #[test]
    fn bilinear_is_exact_on_cell_edges() {
        let data = gradient_grid();
        // On the shared edge between two cells (integer column, fractional row).
        assert_eq!(bilinear(&data, 4, 4, 1.5, 2.0), 17.0);
    }

    #[test]
    fn bilinear_clamps_out_of_range_coordinates() {
        let data = gradient_grid();
        assert_eq!(bilinear(&data, 4, 4, -1.0, -1.0), 0.0);
        assert_eq!(bilinear(&data, 4, 4, 10.0, 10.0), 33.0);
    }

    // --- column-count-varies handling ----------------------------------

    /// Fills a synthetic tile whose values are an exact linear function of
    /// lat/lon, so `Tile::sample` should reconstruct that function exactly
    /// (up to f32 rounding) regardless of the tile's resolution.
    fn linear_tile(lat_floor: i32, lon_floor: i32, width: usize, height: usize) -> Tile {
        let elev = |lat: f64, lon: f64| {
            ((lat - lat_floor as f64) * 1000.0 + (lon - lon_floor as f64) * 100.0) as f32
        };
        let mut data = vec![0.0; width * height];
        for r in 0..height {
            let lat = (lat_floor + 1) as f64 - (r as f64 + 0.5) / height as f64;
            for c in 0..width {
                let lon = lon_floor as f64 + (c as f64 + 0.5) / width as f64;
                data[r * width + c] = elev(lat, lon);
            }
        }
        Tile {
            width,
            height,
            data,
        }
    }

    #[test]
    fn column_count_varies_by_latitude_band_but_samples_consistently() {
        // 3600-style (below 50N) vs. 2400-style (50-60N) column density,
        // scaled down for a fast test while keeping the same width/height
        // ratio (2400/3600 = 2/3).
        let dense = linear_tile(49, 6, 36, 36);
        let sparse = linear_tile(49, 6, 24, 36);

        let lat = 49.4;
        let lon = 6.6;
        let expected = ((lat - 49.0) * 1000.0 + (lon - 6.0) * 100.0) as f32;

        let got_dense = dense.sample(49, 6, lat, lon);
        let got_sparse = sparse.sample(49, 6, lat, lon);

        assert!(
            (got_dense - expected).abs() < 0.5,
            "{got_dense} vs {expected}"
        );
        assert!(
            (got_sparse - expected).abs() < 0.5,
            "{got_sparse} vs {expected}"
        );
    }

    // --- median smoothing ------------------------------------------------

    #[test]
    fn median_smoothing_flattens_a_single_outlier() {
        // Vertices 30m apart; a spike at index 2 sits well within every
        // neighbour's +-75m window and should be smoothed away.
        let cumdist = vec![0.0, 30.0, 60.0, 90.0, 120.0, 150.0];
        let raw = vec![100.0, 100.0, 500.0, 100.0, 100.0, 100.0];
        let smoothed = median_smooth(&cumdist, &raw);
        assert_eq!(smoothed[2], 100.0);
        assert_eq!(smoothed, vec![100.0, 100.0, 100.0, 100.0, 100.0, 100.0]);
    }

    #[test]
    fn median_smoothing_pins_endpoints_so_edge_nets_telescope() {
        // A steady climb with vertices 30m apart: every interior window is
        // one-sided-truncated near the ends, which used to pull the first
        // and last values toward the interior and shave real ascent off the
        // edge. With pinned endpoints the smoothed net must equal the raw
        // endpoint delta exactly.
        let cumdist = vec![0.0, 30.0, 60.0, 90.0, 120.0, 150.0];
        let raw = vec![100.0, 110.0, 120.0, 130.0, 140.0, 150.0];
        let smoothed = median_smooth(&cumdist, &raw);
        assert_eq!(smoothed[0], 100.0);
        assert_eq!(smoothed[5], 150.0);
        let net: f32 = smoothed.windows(2).map(|w| w[1] - w[0]).sum();
        assert_eq!(net, raw[5] - raw[0]);
    }

    #[test]
    fn median_smoothing_leaves_a_monotone_ramp_unchanged() {
        // Vertices far enough apart (200m) that each window is just the
        // vertex itself, so a monotone ramp survives smoothing unchanged.
        let cumdist = vec![0.0, 200.0, 400.0, 600.0];
        let raw = vec![100.0, 120.0, 140.0, 160.0];
        assert_eq!(median_smooth(&cumdist, &raw), raw);
    }

    // --- ascent/descent aggregation on a synthetic model -----------------

    // Rows chosen 5 apart in a 40-row tile (~2.8 km/row) so consecutive
    // vertices land ~14 km apart -- far outside the 150m smoothing window,
    // isolating the ascent/descent aggregation from the smoothing step.
    const HILL_HEIGHT: usize = 40;
    const HILL_ROWS: [usize; 4] = [30, 25, 20, 15];
    const HILL_VALUES: [f32; 4] = [100.0, 150.0, 120.0, 100.0];

    /// The exact latitude of pixel row `row`'s center in a tile of
    /// `HILL_HEIGHT` rows covering `[49,50)`, per `Tile::sample`'s
    /// pixel-center formula -- shared by the model and tile builders below
    /// so a vertex always lands exactly on the pixel it's meant to test.
    fn hill_row_lat(row: usize) -> f32 {
        (50.0 - (row as f64 + 0.5) / HILL_HEIGHT as f64) as f32
    }

    fn hill_model() -> RegionGraphModel {
        let lon = 6.5;
        let geometry: Vec<GeomVertex> = HILL_ROWS
            .iter()
            .map(|&row| geom(hill_row_lat(row), lon))
            .collect();
        RegionGraphModel {
            nodes: vec![
                NodeRecord {
                    lat: hill_row_lat(HILL_ROWS[0]),
                    lon,
                },
                NodeRecord {
                    lat: hill_row_lat(*HILL_ROWS.last().unwrap()),
                    lon,
                },
            ],
            csr_first_edge: vec![0, 1, 1],
            edges: vec![EdgeHot {
                target: 1,
                length_m: 600.0,
                speed_kmh: 50.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 3,
                guide_flags: 0,
                _pad: [0; 2],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset: 0,
                geom_count: geometry.len() as u32,
            }],
            ch_order: vec![0, 1],
            geometry,
            snap_grid: SnapGridModel {
                min_lat: hill_row_lat(HILL_ROWS[0]),
                min_lon: lon,
                cell_size_deg: 0.1,
                n_rows: 1,
                n_cols: 1,
                cell_offsets: vec![0, 2],
                node_ids: vec![0, 1],
            },
            ..Default::default()
        }
    }

    /// A one-column tile covering `[49,50)x[6,7)` whose elevation is a hill
    /// along latitude: 100 -> 150 -> 120 -> 100 m at the rows `hill_model`'s
    /// vertices sample.
    fn hill_tile_store() -> TileStore {
        let mut data = vec![100.0; HILL_HEIGHT];
        for (&row, &val) in HILL_ROWS.iter().zip(HILL_VALUES.iter()) {
            data[row] = val;
        }
        let mut tiles = HashMap::new();
        tiles.insert(
            (49, 6),
            Some(Tile {
                width: 1,
                height: HILL_HEIGHT,
                data,
            }),
        );
        TileStore::from_tiles(tiles)
    }

    #[test]
    fn hill_edge_gets_both_ascent_and_descent_and_writes_elev_m() {
        let mut model = hill_model();
        let mut store = hill_tile_store();
        let stats = run(&mut model, &mut store).unwrap();

        assert_eq!(stats.edges_updated, 1);
        assert_eq!(stats.vertices_sampled, 4);
        let edge = &model.edges[0];
        assert!(
            edge.ascent_m > 0.0,
            "expected ascent, got {}",
            edge.ascent_m
        );
        assert!(
            edge.descent_m > 0.0,
            "expected descent, got {}",
            edge.descent_m
        );
        // Up 50, down 30, up... down 20: ascent 50, descent 50.
        assert!((edge.ascent_m - 50.0).abs() < 1.0);
        assert!((edge.descent_m - 50.0).abs() < 1.0);

        let elevs: Vec<i16> = model.geometry.iter().map(|v| v.elev_m).collect();
        assert_eq!(elevs, vec![100, 150, 120, 100]);
    }

    #[test]
    fn edge_with_fewer_than_two_geometry_points_leaves_ascent_descent_zero() {
        let mut model = hill_model();
        model.edges[0].geom_count = 1;
        let mut store = hill_tile_store();
        run(&mut model, &mut store).unwrap();
        assert_eq!(model.edges[0].ascent_m, 0.0);
        assert_eq!(model.edges[0].descent_m, 0.0);
    }

    // --- missing tile (ocean) path -----------------------------------

    #[test]
    fn missing_tile_samples_as_zero_without_error() {
        let mut tiles = HashMap::new();
        tiles.insert((49, 6), None);
        let mut store = TileStore::from_tiles(tiles);
        let v = store.sample(49.5, 6.5).unwrap();
        assert_eq!(v, 0.0);
        assert_eq!(store.tiles_fetched, 0);
    }

    // --- cache marker-file behaviour, offline ---------------------------

    #[test]
    fn cached_missing_marker_short_circuits_without_network() {
        let dir = tempdir().unwrap();
        let id = tile_id(49, 6);
        fs::write(dir.path().join(format!("{id}.missing")), []).unwrap();

        let mut store = TileStore::new(dir.path().to_path_buf());
        let v = store.sample(49.5, 6.5).unwrap();
        assert_eq!(v, 0.0);
        assert_eq!(store.tiles_missing, 1);
        assert_eq!(store.tiles_fetched, 0);
        assert_eq!(store.tiles_cache_hits, 0);
    }
}
