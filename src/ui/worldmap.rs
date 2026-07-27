//! Land/water lookup for the region globe (`ui::draw_globe`).
//!
//! `worldmap.txt` is an equirectangular land bitmap (`#` = land, `.` = water), 144×72 cells
//! (2.5° per cell). Generated offline (not at build time — no network/geo deps in the crate
//! itself) from Natural Earth's public-domain 110m-resolution land polygons
//! (`ne_110m_land.geojson`), rasterized with a 2×2-supersampled point-in-polygon test per cell.
//! An earlier version, built from a sparser coastline point cloud via flood-fill, left large gaps
//! (most of Russia/Siberia was missing) — this one is a proper polygon dataset, so continents come
//! out as real, unbroken landmasses rather than fragments.

use std::sync::OnceLock;

pub const MASK_W: usize = 144;
pub const MASK_H: usize = 72;

const MASK: &str = include_str!("worldmap.txt");

/// The bitmap unpacked to a flat `MASK_W * MASK_H` grid on first use.
fn grid() -> &'static Vec<bool> {
    static GRID: OnceLock<Vec<bool>> = OnceLock::new();
    GRID.get_or_init(|| {
        let mut cells = vec![false; MASK_W * MASK_H];
        for (r, line) in MASK.lines().enumerate().take(MASK_H) {
            for (c, ch) in line.chars().enumerate().take(MASK_W) {
                cells[r * MASK_W + c] = ch == '#';
            }
        }
        cells
    })
}

/// Whether the given latitude/longitude (degrees) falls on land.
pub fn is_land(lat: f64, lon: f64) -> bool {
    let col = ((((lon + 180.0) / 360.0) * MASK_W as f64).floor() as isize).rem_euclid(MASK_W as isize) as usize;
    let row = (((90.0 - lat) / 180.0) * MASK_H as f64).floor().clamp(0.0, (MASK_H - 1) as f64) as usize;
    grid()[row * MASK_W + col]
}
