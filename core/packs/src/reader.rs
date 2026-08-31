//! Zero-copy `.rpack` reader: mmaps the whole file and hands out `&[T]`
//! slices straight into the mapping via bytemuck casts.

use std::mem::size_of;
use std::path::Path;

use memmap2::Mmap;

use crate::error::RpackError;
use crate::format::{
    align_up, DestSign, EdgeAttr, EdgeHot, ExitRef, GeomVertex, HeaderFixed, NodeRecord,
    SectionEntry, SnapGridHeader, ALIGN, CH_MIDDLE_NODE_NONE, FORMAT_MAJOR, GUIDE_NONE, MAGIC,
    REGION_NAME_LEN, SECTION_CH_ORDER, SECTION_CSR, SECTION_DEST_SIGNS, SECTION_EDGES_HOT,
    SECTION_EDGE_ATTRS, SECTION_EDGE_GUIDE, SECTION_EXIT_REFS, SECTION_GEOMETRY, SECTION_NODES,
    SECTION_REVERSE_CSR, SECTION_REVERSE_EDGES, SECTION_SNAP_GRID, SECTION_STRING_BLOB,
    SECTION_STRING_OFFSETS,
};

/// Whether `[a_offset, a_offset + a_len)` and `[b_offset, b_offset + b_len)`
/// intersect. A zero-length range never overlaps anything at its own
/// boundary, only strictly inside another range.
fn ranges_overlap(a_offset: u64, a_len: u64, b_offset: u64, b_len: u64) -> bool {
    a_offset < b_offset + b_len && b_offset < a_offset + a_len
}

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
    has_guidance: bool,
}

/// Format 2.0 sections, required only when `has_guidance()` (major >= 2).
const GUIDANCE_SECTIONS: &[(u32, usize)] = &[
    (SECTION_EDGE_ATTRS, size_of::<EdgeAttr>()),
    (SECTION_DEST_SIGNS, size_of::<DestSign>()),
    (SECTION_EXIT_REFS, size_of::<ExitRef>()),
];

/// The fixed-element sections, keyed by section id, element size in bytes,
/// and whether the reader requires them to be present.
const FIXED_ELEMENT_SECTIONS: &[(u32, usize)] = &[
    (SECTION_NODES, size_of::<NodeRecord>()),
    (SECTION_CSR, size_of::<u32>()),
    (SECTION_EDGES_HOT, size_of::<EdgeHot>()),
    (SECTION_CH_ORDER, size_of::<u32>()),
    (SECTION_GEOMETRY, size_of::<GeomVertex>()),
    (SECTION_REVERSE_CSR, size_of::<u32>()),
    (SECTION_REVERSE_EDGES, size_of::<u32>()),
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
        // Format 2.0 (wayfinder #65) added guidance sections after every v1
        // section, so a v1 file (no guidance) and a v2 file (guidance
        // present) are both byte-valid to this reader; only a major beyond
        // what this reader knows about is rejected.
        if header.format_major != 1 && header.format_major != FORMAT_MAJOR {
            return Err(RpackError::UnsupportedVersion {
                major: header.format_major,
            });
        }
        let has_guidance = header.format_major >= 2;

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

        // Reject duplicate section ids and sections overlapping each other
        // or the header/section table (M-04). O(section_count^2), which is
        // cheap since a pack has a handful of sections, not O(nodes) or
        // O(edges) -- this must never page in an index array.
        for i in 0..sections.len() {
            for j in (i + 1)..sections.len() {
                if sections[i].id == sections[j].id {
                    return Err(RpackError::DuplicateSection {
                        section_id: sections[i].id,
                    });
                }
                if ranges_overlap(
                    sections[i].offset,
                    sections[i].len_bytes,
                    sections[j].offset,
                    sections[j].len_bytes,
                ) {
                    return Err(RpackError::OverlappingSections {
                        section_id_a: sections[i].id,
                        section_id_b: sections[j].id,
                    });
                }
            }
        }
        for s in &sections {
            if ranges_overlap(s.offset, s.len_bytes, 0, table_end) {
                return Err(RpackError::SectionOverlapsHeader { section_id: s.id });
            }
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
        // Cross-section length checks: now that every section's shape is
        // known, confirm the arrays that must agree on node/edge counts
        // actually do, rather than trusting each section in isolation.
        let n_nodes = sections
            .iter()
            .find(|s| s.id == SECTION_NODES)
            .map(|s| s.len_bytes / size_of::<NodeRecord>() as u64)
            .expect("presence checked above");
        let n_edges = sections
            .iter()
            .find(|s| s.id == SECTION_EDGES_HOT)
            .map(|s| s.len_bytes / size_of::<EdgeHot>() as u64)
            .expect("presence checked above");
        let check_len = |id: u32, expected: u64| -> Result<(), RpackError> {
            let meta = sections
                .iter()
                .find(|s| s.id == id)
                .expect("presence checked above");
            let got = meta.len_bytes / size_of::<u32>() as u64;
            if got != expected {
                return Err(RpackError::Validation(format!(
                    "section {id} has {got} elements, expected {expected}"
                )));
            }
            Ok(())
        };
        check_len(SECTION_CSR, n_nodes + 1)?;
        check_len(SECTION_CH_ORDER, n_nodes)?;
        check_len(SECTION_REVERSE_CSR, n_nodes + 1)?;
        check_len(SECTION_REVERSE_EDGES, n_edges)?;

        // Format 2.0 guidance sections (wayfinder #65): only required for
        // major >= 2. Shape checks only -- full monotonicity and
        // cross-reference validity are `verify_structure`'s job, same as
        // every other section here.
        if has_guidance {
            let string_offsets_meta = sections
                .iter()
                .find(|s| s.id == SECTION_STRING_OFFSETS)
                .ok_or(RpackError::MissingSection {
                    section_id: SECTION_STRING_OFFSETS,
                })?;
            if !string_offsets_meta.len_bytes.is_multiple_of(4)
                || string_offsets_meta.len_bytes < 8
            {
                return Err(RpackError::SectionSizeMismatch {
                    section_id: SECTION_STRING_OFFSETS,
                    len_bytes: string_offsets_meta.len_bytes,
                    elem_size: size_of::<u32>() as u64,
                });
            }
            let string_blob_meta = sections
                .iter()
                .find(|s| s.id == SECTION_STRING_BLOB)
                .ok_or(RpackError::MissingSection {
                    section_id: SECTION_STRING_BLOB,
                })?;

            // Two 4-byte reads: the first and last string offset. Full
            // monotonicity is deep-verify's job; per-access checks in
            // `string()` make lookups safe regardless of what's in between.
            let first_offset_bytes = string_offsets_meta.offset as usize;
            let first: u32 = bytemuck::pod_read_unaligned(
                &mmap[first_offset_bytes..first_offset_bytes + 4],
            );
            if first != 0 {
                return Err(RpackError::Validation(
                    "string_offsets[0] must be 0".into(),
                ));
            }
            let last_offset_bytes =
                (string_offsets_meta.offset + string_offsets_meta.len_bytes - 4) as usize;
            let last: u32 =
                bytemuck::pod_read_unaligned(&mmap[last_offset_bytes..last_offset_bytes + 4]);
            if last as u64 != string_blob_meta.len_bytes {
                return Err(RpackError::Validation(format!(
                    "string_offsets' last entry {last} does not match string_blob length {}",
                    string_blob_meta.len_bytes
                )));
            }

            for &(id, elem_size) in GUIDANCE_SECTIONS {
                let meta = sections
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or(RpackError::MissingSection { section_id: id })?;
                if id == SECTION_EDGE_ATTRS && meta.len_bytes < elem_size as u64 {
                    return Err(RpackError::SectionSizeMismatch {
                        section_id: id,
                        len_bytes: meta.len_bytes,
                        elem_size: elem_size as u64,
                    });
                }
                if !meta.len_bytes.is_multiple_of(elem_size as u64) {
                    return Err(RpackError::SectionSizeMismatch {
                        section_id: id,
                        len_bytes: meta.len_bytes,
                        elem_size: elem_size as u64,
                    });
                }
            }
            sections
                .iter()
                .find(|s| s.id == SECTION_EDGE_GUIDE)
                .ok_or(RpackError::MissingSection {
                    section_id: SECTION_EDGE_GUIDE,
                })?;
            check_len(SECTION_EDGE_GUIDE, n_edges)?;
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
            has_guidance,
        })
    }

    pub fn format_version(&self) -> (u16, u16) {
        (self.format_major, self.format_minor)
    }

    /// Whether this pack carries format 2.0 guidance sections (wayfinder
    /// #65): the string table, edge attrs/guide, dest signs, and exit refs.
    /// `false` for a format 1.x pack, in which case every guidance accessor
    /// returns an empty slice / `None`.
    pub fn has_guidance(&self) -> bool {
        self.has_guidance
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

    /// Deep, opt-in validation of every cross-section index invariant the
    /// CH search and geometry lookups rely on: the forward and reverse CSR
    /// row indices are monotonically nondecreasing and end at the edge
    /// count, every edge's target is a valid node id, every baked
    /// reverse-adjacency entry is a valid edge id, every CH order value is
    /// a valid node id, and every edge's geometry range stays inside the
    /// geometry section.
    ///
    /// This is `O(nodes + edges)`, unlike `open`'s O(section-count)
    /// structural checks, so it is never run automatically. Use it for
    /// repair/QA paths on a sideloaded or otherwise untrusted file --
    /// `open` guarantees a well-formed file layout, `verify_checksums`
    /// (run at install time) guarantees content wasn't corrupted in
    /// transit, and `verify_structure` is the deep check beyond either of
    /// those for when you need it.
    pub fn verify_structure(&self) -> Result<(), RpackError> {
        let n_nodes = self.node_count() as u64;
        let n_edges = self.edges().len() as u64;
        let n_geom = self.geometry().len() as u64;

        let check_csr = |name: &str, csr: &[u32]| -> Result<(), RpackError> {
            for w in csr.windows(2) {
                if w[1] < w[0] {
                    return Err(RpackError::Validation(format!(
                        "{name} is not monotonically nondecreasing"
                    )));
                }
            }
            match csr.last() {
                Some(&last) if last as u64 == n_edges => Ok(()),
                _ => Err(RpackError::Validation(format!(
                    "{name}'s last entry does not match the edge count {n_edges}"
                ))),
            }
        };
        check_csr("csr_first_edge", self.csr_first_edge())?;
        check_csr("reverse_csr", self.reverse_csr())?;

        for (i, e) in self.edges().iter().enumerate() {
            if e.target as u64 >= n_nodes {
                return Err(RpackError::Validation(format!(
                    "edge {i} targets out-of-range node {}",
                    e.target
                )));
            }
            let geom_end = e.geom_offset as u64 + e.geom_count as u64;
            if geom_end > n_geom {
                return Err(RpackError::Validation(format!(
                    "edge {i} geometry range [{}, {geom_end}) exceeds geometry section of {n_geom}",
                    e.geom_offset
                )));
            }
        }

        for (i, &edge_id) in self.reverse_edge_ids().iter().enumerate() {
            if edge_id as u64 >= n_edges {
                return Err(RpackError::Validation(format!(
                    "reverse edge index {i} references out-of-range edge {edge_id}"
                )));
            }
        }

        for (i, &rank) in self.ch_order().iter().enumerate() {
            if rank as u64 >= n_nodes {
                return Err(RpackError::Validation(format!(
                    "ch_order entry {i} has out-of-range value {rank}"
                )));
            }
        }

        // Format 2.0 guidance (wayfinder #65): v1 packs have none of this,
        // so the old checks above are the whole story for them.
        if self.has_guidance() {
            let string_offsets = self.string_offsets();
            for w in string_offsets.windows(2) {
                if w[1] < w[0] {
                    return Err(RpackError::Validation(
                        "string_offsets is not monotonically nondecreasing".into(),
                    ));
                }
            }
            if let Err(e) = std::str::from_utf8(self.string_blob()) {
                return Err(RpackError::Validation(format!(
                    "string_blob is not valid UTF-8 at byte offset {}",
                    e.valid_up_to()
                )));
            }
            let n_strings = string_offsets.len() as u64 - 1;

            let edge_attrs = self.edge_attrs();
            let n_attrs = edge_attrs.len() as u64;
            match edge_attrs.first() {
                Some(&EdgeAttr { name_id: 0, ref_id: 0 }) => {}
                _ => {
                    return Err(RpackError::Validation(
                        "edge_attrs[0] must be {name_id: 0, ref_id: 0}".into(),
                    ))
                }
            }
            for (i, attr) in edge_attrs.iter().enumerate() {
                if attr.name_id as u64 >= n_strings || attr.ref_id as u64 >= n_strings {
                    return Err(RpackError::Validation(format!(
                        "edge_attrs[{i}] references out-of-range string id"
                    )));
                }
            }

            for (i, (&guide, edge)) in self.edge_guide().iter().zip(self.edges()).enumerate() {
                let is_shortcut = edge.ch_middle_node != CH_MIDDLE_NODE_NONE;
                if is_shortcut {
                    if guide != GUIDE_NONE {
                        return Err(RpackError::Validation(format!(
                            "edge_guide[{i}] is {guide}, expected GUIDE_NONE for a shortcut edge"
                        )));
                    }
                } else if guide as u64 >= n_attrs {
                    return Err(RpackError::Validation(format!(
                        "edge_guide[{i}] references out-of-range attr {guide}"
                    )));
                }
            }

            let mut prev_slot: Option<u32> = None;
            for (i, sign) in self.dest_signs().iter().enumerate() {
                if let Some(prev) = prev_slot {
                    if sign.edge_slot <= prev {
                        return Err(RpackError::Validation(
                            "dest_signs is not strictly increasing by edge_slot".into(),
                        ));
                    }
                }
                prev_slot = Some(sign.edge_slot);
                let Some(edge) = self.edges().get(sign.edge_slot as usize) else {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] references out-of-range edge_slot {}",
                        sign.edge_slot
                    )));
                };
                if edge.ch_middle_node != CH_MIDDLE_NODE_NONE {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] edge_slot {} is a shortcut, not an original edge",
                        sign.edge_slot
                    )));
                }
                if sign.dest_id as u64 >= n_strings
                    || sign.dest_ref_id as u64 >= n_strings
                    || sign.junction_ref_id as u64 >= n_strings
                {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] references out-of-range string id"
                    )));
                }
            }

            let mut prev_node: Option<u32> = None;
            for (i, exit) in self.exit_refs().iter().enumerate() {
                if let Some(prev) = prev_node {
                    if exit.node_id <= prev {
                        return Err(RpackError::Validation(
                            "exit_refs is not strictly increasing by node_id".into(),
                        ));
                    }
                }
                prev_node = Some(exit.node_id);
                if exit.node_id as u64 >= n_nodes {
                    return Err(RpackError::Validation(format!(
                        "exit_refs[{i}] references out-of-range node {}",
                        exit.node_id
                    )));
                }
                if exit.ref_id as u64 >= n_strings {
                    return Err(RpackError::Validation(format!(
                        "exit_refs[{i}] references out-of-range string id {}",
                        exit.ref_id
                    )));
                }
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

    /// Bytes of section `id`, or `None` if it isn't present -- for the
    /// format 2.0 guidance sections, which a v1 pack never carries.
    /// `section_bytes` keeps its existing "must be present" contract; this
    /// is the opt-in counterpart guidance accessors use instead.
    fn section_bytes_opt(&self, id: u32) -> Option<&[u8]> {
        let meta = self.sections.iter().find(|s| s.id == id)?;
        Some(&self.mmap[meta.offset as usize..(meta.offset + meta.len_bytes) as usize])
    }

    /// Format 2.0 guidance (wayfinder #65): the interned string table's
    /// offsets, length `n_strings + 1`. Empty on a v1 pack.
    pub fn string_offsets(&self) -> &[u32] {
        self.section_bytes_opt(SECTION_STRING_OFFSETS)
            .map(bytemuck::cast_slice)
            .unwrap_or(&[])
    }

    /// Format 2.0 guidance: the interned string table's UTF-8 blob. Empty on
    /// a v1 pack.
    pub fn string_blob(&self) -> &[u8] {
        self.section_bytes_opt(SECTION_STRING_BLOB).unwrap_or(&[])
    }

    /// Guidance string by id. `None` for v1 packs, out-of-range ids, inverted
    /// offset pairs, out-of-blob ranges, or invalid UTF-8 (malformed-pack
    /// robustness, same spirit as `snap()`'s per-node-id tolerance).
    pub fn string(&self, id: u32) -> Option<&str> {
        let offsets = self.string_offsets();
        let i = id as usize;
        if i + 1 >= offsets.len() {
            return None;
        }
        let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
        if start > end {
            return None;
        }
        let blob = self.string_blob();
        let bytes = blob.get(start..end)?;
        std::str::from_utf8(bytes).ok()
    }

    /// Format 2.0 guidance: unique (name, ref) pairs; entry 0 is always
    /// `{0, 0}` (unnamed). Empty on a v1 pack.
    pub fn edge_attrs(&self) -> &[EdgeAttr] {
        self.section_bytes_opt(SECTION_EDGE_ATTRS)
            .map(bytemuck::cast_slice)
            .unwrap_or(&[])
    }

    /// Format 2.0 guidance: one entry per `edges()` slot, indexing
    /// `edge_attrs()` (`GUIDE_NONE` for shortcut edges). Empty on a v1 pack.
    pub fn edge_guide(&self) -> &[u32] {
        self.section_bytes_opt(SECTION_EDGE_GUIDE)
            .map(bytemuck::cast_slice)
            .unwrap_or(&[])
    }

    /// Format 2.0 guidance: sparse destination signage, sorted by
    /// `edge_slot`. Empty on a v1 pack.
    pub fn dest_signs(&self) -> &[DestSign] {
        self.section_bytes_opt(SECTION_DEST_SIGNS)
            .map(bytemuck::cast_slice)
            .unwrap_or(&[])
    }

    /// Format 2.0 guidance: sparse motorway-junction exit refs, sorted by
    /// `node_id`. Empty on a v1 pack.
    pub fn exit_refs(&self) -> &[ExitRef] {
        self.section_bytes_opt(SECTION_EXIT_REFS)
            .map(bytemuck::cast_slice)
            .unwrap_or(&[])
    }

    /// Binary search by `edge_slot`. `None` on a v1 pack or no entry.
    pub fn dest_sign_for_edge(&self, edge_slot: u32) -> Option<&DestSign> {
        let signs = self.dest_signs();
        signs
            .binary_search_by_key(&edge_slot, |s| s.edge_slot)
            .ok()
            .map(|i| &signs[i])
    }

    /// Binary search by `node_id`, returning the ref string id. `None` on a
    /// v1 pack or no entry.
    pub fn exit_ref_for_node(&self, node_id: u32) -> Option<u32> {
        let refs = self.exit_refs();
        refs.binary_search_by_key(&node_id, |r| r.node_id)
            .ok()
            .map(|i| refs[i].ref_id)
    }

    /// CSR row index into `reverse_edge_ids()`, length `node_count() + 1`.
    pub fn reverse_csr(&self) -> &[u32] {
        bytemuck::cast_slice(self.section_bytes(SECTION_REVERSE_CSR))
    }

    fn reverse_edge_ids(&self) -> &[u32] {
        bytemuck::cast_slice(self.section_bytes(SECTION_REVERSE_EDGES))
    }

    /// Indices into `edges()` for `node_id`'s incoming edges (edges whose
    /// `target` is `node_id`), via the baked reverse-adjacency CSR. `None`
    /// if `node_id` is out of range.
    pub fn reverse_edge_ids_for(&self, node_id: u32) -> Option<&[u32]> {
        let csr = self.reverse_csr();
        let i = node_id as usize;
        if i + 1 >= csr.len() {
            return None;
        }
        Some(&self.reverse_edge_ids()[csr[i] as usize..csr[i + 1] as usize])
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
