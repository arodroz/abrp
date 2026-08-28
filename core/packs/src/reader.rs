//! Zero-copy `.rpack` reader: mmaps the whole file and hands out `&[T]`
//! slices straight into the mapping via bytemuck casts.

use std::mem::size_of;
use std::path::Path;

use memmap2::Mmap;

use crate::error::RpackError;
use crate::format::{
    align_up, EdgeHot, GeomVertex, HeaderFixed, NodeRecord, SectionEntry, SnapGridHeader, ALIGN,
    FORMAT_MAJOR, MAGIC, REGION_NAME_LEN, SECTION_CH_ORDER, SECTION_CSR, SECTION_EDGES_HOT,
    SECTION_GEOMETRY, SECTION_NODES, SECTION_SNAP_GRID,
};

struct SectionMeta {
    id: u32,
    offset: u64,
    len_bytes: u64,
    crc32: u32,
}

pub struct Rpack {
    mmap: Mmap,
    format_major: u16,
    format_minor: u16,
    osm_snapshot_epoch: u64,
    region_id: u32,
    region_name: String,
    sections: Vec<SectionMeta>,
}

/// The fixed-element sections, keyed by section id, element size in bytes,
/// and whether the reader requires them to be present.
const FIXED_ELEMENT_SECTIONS: &[(u32, usize)] = &[
    (SECTION_NODES, size_of::<NodeRecord>()),
    (SECTION_CSR, size_of::<u32>()),
    (SECTION_EDGES_HOT, size_of::<EdgeHot>()),
    (SECTION_CH_ORDER, size_of::<u32>()),
    (SECTION_GEOMETRY, size_of::<GeomVertex>()),
];

impl Rpack {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, RpackError> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let len = mmap.len() as u64;

        let header_size = size_of::<HeaderFixed>() as u64;
        if len < header_size {
            return Err(RpackError::Truncated {
                needed: header_size,
                available: len,
            });
        }
        let header: HeaderFixed = bytemuck::pod_read_unaligned(&mmap[0..header_size as usize]);

        if header.magic != MAGIC {
            return Err(RpackError::BadMagic);
        }
        if header.format_major != FORMAT_MAJOR {
            return Err(RpackError::UnsupportedVersion {
                major: header.format_major,
            });
        }

        let entry_size = size_of::<SectionEntry>() as u64;
        let table_len = header.section_count as u64 * entry_size;
        let table_end = header_size
            .checked_add(table_len)
            .ok_or(RpackError::Truncated {
                needed: u64::MAX,
                available: len,
            })?;
        if table_end > len {
            return Err(RpackError::Truncated {
                needed: table_end,
                available: len,
            });
        }

        let mut sections = Vec::with_capacity(header.section_count as usize);
        for i in 0..header.section_count as u64 {
            let start = (header_size + i * entry_size) as usize;
            let entry: SectionEntry =
                bytemuck::pod_read_unaligned(&mmap[start..start + entry_size as usize]);

            if !entry.offset.is_multiple_of(ALIGN) {
                return Err(RpackError::Misaligned {
                    offset: entry.offset,
                });
            }
            let end = entry.offset.checked_add(entry.len_bytes).ok_or(
                RpackError::SectionOutOfBounds {
                    section_id: entry.section_id,
                },
            )?;
            if end > len {
                return Err(RpackError::SectionOutOfBounds {
                    section_id: entry.section_id,
                });
            }

            sections.push(SectionMeta {
                id: entry.section_id,
                offset: entry.offset,
                len_bytes: entry.len_bytes,
                crc32: entry.crc32,
            });
        }

        for &(id, elem_size) in FIXED_ELEMENT_SECTIONS {
            let meta = sections
                .iter()
                .find(|s| s.id == id)
                .ok_or(RpackError::MissingSection { section_id: id })?;
            if !meta.len_bytes.is_multiple_of(elem_size as u64) {
                return Err(RpackError::SectionSizeMismatch {
                    section_id: id,
                    len_bytes: meta.len_bytes,
                    elem_size: elem_size as u64,
                });
            }
        }
        let snap_grid_meta = sections.iter().find(|s| s.id == SECTION_SNAP_GRID).ok_or(
            RpackError::MissingSection {
                section_id: SECTION_SNAP_GRID,
            },
        )?;
        let snap_grid_header_size = size_of::<SnapGridHeader>() as u64;
        if snap_grid_meta.len_bytes < snap_grid_header_size {
            return Err(RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: snap_grid_header_size,
            });
        }

        // The grid header's n_rows/n_cols and the cell_offsets/node_ids
        // arrays that follow it are attacker-controlled bytes read straight
        // from the mmap. Validate their shape here (cheap: touches only the
        // header and the offsets array, never node_ids) so `snap()` can
        // trust cell_offsets bounds later without range-checking every node
        // id (which would fault in the whole node_ids array and defeat
        // paging).
        let snap_grid_header: SnapGridHeader = bytemuck::pod_read_unaligned(
            &mmap[snap_grid_meta.offset as usize
                ..(snap_grid_meta.offset + snap_grid_header_size) as usize],
        );
        let n_cells = (snap_grid_header.n_rows as u64)
            .checked_mul(snap_grid_header.n_cols as u64)
            .ok_or(RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: size_of::<u32>() as u64,
            })?;
        let offsets_len = n_cells
            .checked_add(1)
            .and_then(|n| n.checked_mul(size_of::<u32>() as u64))
            .ok_or(RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: size_of::<u32>() as u64,
            })?;
        let min_required = snap_grid_header_size.checked_add(offsets_len).ok_or(
            RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: size_of::<u32>() as u64,
            },
        )?;
        if snap_grid_meta.len_bytes < min_required {
            return Err(RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: min_required,
            });
        }
        let node_id_bytes = snap_grid_meta.len_bytes - min_required;
        if !node_id_bytes.is_multiple_of(size_of::<u32>() as u64) {
            return Err(RpackError::SectionSizeMismatch {
                section_id: SECTION_SNAP_GRID,
                len_bytes: snap_grid_meta.len_bytes,
                elem_size: size_of::<u32>() as u64,
            });
        }
        let node_id_count = node_id_bytes / size_of::<u32>() as u64;

        let cell_offsets_start = snap_grid_meta.offset + snap_grid_header_size;
        let cell_offsets: &[u32] = bytemuck::cast_slice(
            &mmap[cell_offsets_start as usize..(cell_offsets_start + offsets_len) as usize],
        );
        let mut prev = 0u32;
        for (i, &v) in cell_offsets.iter().enumerate() {
            if i > 0 && v < prev {
                return Err(RpackError::Validation(format!(
                    "snap grid cell_offsets is not monotone at index {i}"
                )));
            }
            prev = v;
        }
        if *cell_offsets
            .last()
            .expect("cell_offsets always has n_cells + 1 >= 1 entries") as u64
            != node_id_count
        {
            return Err(RpackError::Validation(
                "snap grid cell_offsets' last entry does not match the node id array length"
                    .to_string(),
            ));
        }

        let region_name = {
            let bytes = &header.region_name;
            let nul = bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(REGION_NAME_LEN);
            String::from_utf8_lossy(&bytes[..nul]).into_owned()
        };

        Ok(Rpack {
            mmap,
            format_major: header.format_major,
            format_minor: header.format_minor,
            osm_snapshot_epoch: header.osm_snapshot_epoch,
            region_id: header.region_id,
            region_name,
            sections,
        })
    }

    pub fn format_version(&self) -> (u16, u16) {
        (self.format_major, self.format_minor)
    }

    pub fn osm_snapshot_epoch(&self) -> u64 {
        self.osm_snapshot_epoch
    }

    pub fn region_id(&self) -> u32 {
        self.region_id
    }

    pub fn region_name(&self) -> &str {
        &self.region_name
    }

    fn section_meta(&self, id: u32) -> &SectionMeta {
        self.sections
            .iter()
            .find(|s| s.id == id)
            .expect("presence of required sections validated at open")
    }

    fn section_bytes(&self, id: u32) -> &[u8] {
        let meta = self.section_meta(id);
        &self.mmap[meta.offset as usize..(meta.offset + meta.len_bytes) as usize]
    }

    /// Verifies every section's CRC32 against the header's recorded value.
    /// Not run automatically by `open` since it touches every page; call it
    /// explicitly when you want to confirm the file wasn't corrupted.
    pub fn verify_checksums(&self) -> Result<(), RpackError> {
        for meta in &self.sections {
            let bytes = &self.mmap[meta.offset as usize..(meta.offset + meta.len_bytes) as usize];
            if crc32fast::hash(bytes) != meta.crc32 {
                return Err(RpackError::ChecksumMismatch {
                    section_id: meta.id,
                });
            }
        }
        Ok(())
    }

    pub fn nodes(&self) -> &[NodeRecord] {
        bytemuck::cast_slice(self.section_bytes(SECTION_NODES))
    }

    pub fn node_count(&self) -> usize {
        self.nodes().len()
    }

    pub fn csr_first_edge(&self) -> &[u32] {
        bytemuck::cast_slice(self.section_bytes(SECTION_CSR))
    }

    pub fn edges(&self) -> &[EdgeHot] {
        bytemuck::cast_slice(self.section_bytes(SECTION_EDGES_HOT))
    }

    pub fn ch_order(&self) -> &[u32] {
        bytemuck::cast_slice(self.section_bytes(SECTION_CH_ORDER))
    }

    pub fn geometry(&self) -> &[GeomVertex] {
        bytemuck::cast_slice(self.section_bytes(SECTION_GEOMETRY))
    }

    /// The `[start, end)` range into `edges()` for `node_id`'s outgoing
    /// edges, via the CSR row index. `None` if `node_id` is out of range.
    pub fn edge_range(&self, node_id: u32) -> Option<std::ops::Range<usize>> {
        let csr = self.csr_first_edge();
        let i = node_id as usize;
        if i + 1 >= csr.len() {
            return None;
        }
        Some(csr[i] as usize..csr[i + 1] as usize)
    }

    /// Outgoing edges for `node_id`, via the CSR row index.
    pub fn edges_for(&self, node_id: u32) -> Option<&[EdgeHot]> {
        let range = self.edge_range(node_id)?;
        Some(&self.edges()[range])
    }

    /// Cold polyline vertices for one edge.
    pub fn geometry_for_edge(&self, edge: &EdgeHot) -> &[GeomVertex] {
        let start = edge.geom_offset as usize;
        let end = start + edge.geom_count as usize;
        &self.geometry()[start..end]
    }

    fn snap_grid_header(&self) -> &SnapGridHeader {
        bytemuck::from_bytes(&self.section_bytes(SECTION_SNAP_GRID)[..size_of::<SnapGridHeader>()])
    }

    fn snap_grid_cell_offsets(&self) -> &[u32] {
        let header = self.snap_grid_header();
        let n_cells = header.n_rows as usize * header.n_cols as usize;
        let start = size_of::<SnapGridHeader>();
        let end = start + (n_cells + 1) * size_of::<u32>();
        bytemuck::cast_slice(&self.section_bytes(SECTION_SNAP_GRID)[start..end])
    }

    fn snap_grid_node_ids(&self) -> &[u32] {
        let header = self.snap_grid_header();
        let n_cells = header.n_rows as usize * header.n_cols as usize;
        let start = size_of::<SnapGridHeader>() + (n_cells + 1) * size_of::<u32>();
        bytemuck::cast_slice(&self.section_bytes(SECTION_SNAP_GRID)[start..])
    }

    /// Nearest-node lookup for `(lat, lon)`, searching the query's grid cell
    /// and its 8 neighbours. Distance is equirectangular (longitude scaled
    /// by `cos(query latitude)`), which is cheap and, over one 0.1deg cell's
    /// span, indistinguishable in ranking from great-circle distance.
    /// Returns `None` if the query cell and its neighbours are all empty
    /// (including queries far outside the pack's coverage).
    pub fn snap(&self, lat: f32, lon: f32) -> Option<u32> {
        let header = self.snap_grid_header();
        if header.n_rows == 0 || header.n_cols == 0 {
            return None;
        }
        let cell_offsets = self.snap_grid_cell_offsets();
        let node_ids = self.snap_grid_node_ids();
        let nodes = self.nodes();

        let col = ((lon - header.min_lon) / header.cell_size_deg).floor() as i64;
        let row = ((lat - header.min_lat) / header.cell_size_deg).floor() as i64;
        let cos_lat = (lat as f64).to_radians().cos();

        let mut best: Option<(u32, f64)> = None;
        for dr in -1..=1i64 {
            for dc in -1..=1i64 {
                let r = row + dr;
                let c = col + dc;
                if r < 0
                    || c < 0
                    || r as u64 >= header.n_rows as u64
                    || c as u64 >= header.n_cols as u64
                {
                    continue;
                }
                let cell_idx = r as usize * header.n_cols as usize + c as usize;
                let start = cell_offsets[cell_idx] as usize;
                let end = cell_offsets[cell_idx + 1] as usize;
                for &node_id in &node_ids[start..end] {
                    // cell_offsets/node_ids shape was validated at open, but
                    // individual node ids are not (checking every one would
                    // fault in the whole node_ids array and defeat paging),
                    // so a malformed pack can still reference an out-of-range
                    // node id here -- skip it rather than panic.
                    let Some(&n) = nodes.get(node_id as usize) else {
                        continue;
                    };
                    let dlat = (n.lat - lat) as f64;
                    let dlon = (n.lon - lon) as f64 * cos_lat;
                    let d2 = dlat * dlat + dlon * dlon;
                    if best.is_none_or(|(_, best_d2)| d2 < best_d2) {
                        best = Some((node_id, d2));
                    }
                }
            }
        }
        best.map(|(id, _)| id)
    }
}

/// Pads `n` to `ALIGN` bytes; exposed so the writer can use the exact same
/// alignment rule the reader validates against.
pub fn alignment_padding(n: u64) -> u64 {
    align_up(n) - n
}
