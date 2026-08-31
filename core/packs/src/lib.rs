//! Region/Map/Charger Pack binary format: writers and mmap readers.
//!
//! This crate defines the `.rpack` Region Pack format (header, section
//! table, and per-section element types) and provides a zero-copy mmap
//! reader. See docs/adr/0007-region-pack-format.md.

pub mod error;
pub mod format;
pub mod reader;

pub use error::RpackError;
pub use format::{
    align_up, DestSign, EdgeAttr, EdgeHot, ExitRef, GeomVertex, HeaderFixed, NodeRecord,
    RegionGraphModel, SectionEntry, SnapGridHeader, SnapGridModel, ALIGN, CH_MIDDLE_NODE_NONE,
    FORMAT_MAJOR, FORMAT_MINOR, GUIDE_CLASS_LIVING_STREET, GUIDE_CLASS_MOTORWAY,
    GUIDE_CLASS_NONE, GUIDE_CLASS_PRIMARY, GUIDE_CLASS_RESIDENTIAL, GUIDE_CLASS_SECONDARY,
    GUIDE_CLASS_TERTIARY, GUIDE_CLASS_TRUNK, GUIDE_CLASS_UNCLASSIFIED, GUIDE_FLAG_LINK,
    GUIDE_FLAG_ROUNDABOUT, GUIDE_NONE, MAGIC, REGION_NAME_LEN, SECTION_CH_ORDER, SECTION_CSR,
    SECTION_DEST_SIGNS, SECTION_EDGES_HOT, SECTION_EDGE_ATTRS, SECTION_EDGE_GUIDE,
    SECTION_EXIT_REFS, SECTION_GEOMETRY, SECTION_NODES, SECTION_REVERSE_CSR,
    SECTION_REVERSE_EDGES, SECTION_SNAP_GRID, SECTION_STRING_BLOB, SECTION_STRING_OFFSETS,
};
pub use reader::{alignment_padding, Rpack};

#[cfg(test)]
mod tests {
    use super::*;
    use format::{EdgeHot as Edge, NodeRecord as Node, SnapGridModel};
    use std::mem::size_of;

    #[test]
    fn header_fixed_is_56_bytes_with_no_implicit_padding() {
        assert_eq!(size_of::<HeaderFixed>(), 56);
    }

    #[test]
    fn section_entry_is_32_bytes_with_no_implicit_padding() {
        assert_eq!(size_of::<SectionEntry>(), 32);
    }

    #[test]
    fn edge_hot_has_no_implicit_padding_beyond_the_explicit_pad_byte() {
        // target(4) + length_m(4) + speed_kmh(4) + ascent_m(4) + descent_m(4)
        // + road_class(1) + guide_flags(1) + _pad(2) + ch_middle_node(4) + geom_offset(4) + geom_count(4)
        assert_eq!(size_of::<EdgeHot>(), 36);
    }

    #[test]
    fn geom_vertex_is_12_bytes() {
        assert_eq!(size_of::<GeomVertex>(), 12);
    }

    #[test]
    fn align_up_rounds_to_next_multiple_of_8() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), 8);
        assert_eq!(align_up(8), 8);
        assert_eq!(align_up(9), 16);
        assert_eq!(align_up(63), 64);
    }

    #[test]
    fn alignment_padding_gives_bytes_needed_to_reach_next_boundary() {
        assert_eq!(alignment_padding(0), 0);
        assert_eq!(alignment_padding(1), 7);
        assert_eq!(alignment_padding(8), 0);
        assert_eq!(alignment_padding(20), 4);
    }

    fn valid_model() -> RegionGraphModel {
        RegionGraphModel {
            nodes: vec![
                Node {
                    lat: 49.5,
                    lon: 6.0,
                },
                Node {
                    lat: 49.6,
                    lon: 6.1,
                },
            ],
            csr_first_edge: vec![0, 1, 1],
            edges: vec![Edge {
                target: 1,
                length_m: 100.0,
                speed_kmh: 50.0,
                ascent_m: 1.0,
                descent_m: 0.0,
                road_class: 3,
                guide_flags: 0,
                _pad: [0; 2],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset: 0,
                geom_count: 2,
            }],
            ch_order: vec![0, 1],
            geometry: vec![
                GeomVertex {
                    lat: 49.5,
                    lon: 6.0,
                    elev_m: 250,
                    _pad: 0,
                },
                GeomVertex {
                    lat: 49.6,
                    lon: 6.1,
                    elev_m: 260,
                    _pad: 0,
                },
            ],
            snap_grid: SnapGridModel {
                min_lat: 49.5,
                min_lon: 6.0,
                cell_size_deg: 0.1,
                n_rows: 1,
                n_cols: 1,
                cell_offsets: vec![0, 2],
                node_ids: vec![0, 1],
            },
            string_offsets: Vec::new(),
            string_blob: Vec::new(),
            edge_attrs: Vec::new(),
            edge_guide: Vec::new(),
            dest_signs: Vec::new(),
            exit_refs: Vec::new(),
        }
    }

    #[test]
    fn valid_model_passes_validation() {
        assert!(valid_model().validate().is_ok());
    }

    #[test]
    fn rejects_csr_with_wrong_length() {
        let mut m = valid_model();
        m.csr_first_edge.pop();
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_csr_not_ending_at_edge_count() {
        let mut m = valid_model();
        *m.csr_first_edge.last_mut().unwrap() = 5;
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_non_monotone_csr() {
        let mut m = valid_model();
        m.csr_first_edge = vec![0, 5, 1];
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_edge_target_out_of_range() {
        let mut m = valid_model();
        m.edges[0].target = 99;
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_edge_ch_middle_node_out_of_range() {
        let mut m = valid_model();
        m.edges[0].ch_middle_node = 99;
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_edge_geometry_range_out_of_bounds() {
        let mut m = valid_model();
        m.edges[0].geom_count = 99;
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_wrong_ch_order_length() {
        let mut m = valid_model();
        m.ch_order.pop();
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_snap_grid_cell_offsets_wrong_length() {
        let mut m = valid_model();
        m.snap_grid.cell_offsets.push(2);
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_snap_grid_referencing_unknown_node() {
        let mut m = valid_model();
        m.snap_grid.node_ids[0] = 99;
        assert!(m.validate().is_err());
    }
}
