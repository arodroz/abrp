//! Streaming `.rpack` writer: pads each section to an 8-byte boundary as it
//! streams it to disk, then rewrites the header + section table in place
//! once every section's offset/len/crc32 is known.

use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::path::Path;

use packs::{
    alignment_padding, EdgeHot, GeomVertex, HeaderFixed, NodeRecord, RegionGraphModel, RpackError,
    SectionEntry, SnapGridHeader, REGION_NAME_LEN, SECTION_CH_ORDER, SECTION_CSR,
    SECTION_EDGES_HOT, SECTION_GEOMETRY, SECTION_NODES, SECTION_SNAP_GRID,
};

/// Pack-level metadata that isn't part of the graph model itself.
pub struct PackMeta {
    pub osm_snapshot_epoch: u64,
    pub region_id: u32,
    pub region_name: String,
}

#[derive(Debug)]
pub enum WriteError {
    Io(std::io::Error),
    Model(RpackError),
    RegionNameTooLong { max: usize, got: usize },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Io(e) => write!(f, "io error: {e}"),
            WriteError::Model(e) => write!(f, "{e}"),
            WriteError::RegionNameTooLong { max, got } => {
                write!(f, "region name is {got} bytes, max is {max}")
            }
        }
    }
}

impl std::error::Error for WriteError {}

impl From<std::io::Error> for WriteError {
    fn from(e: std::io::Error) -> Self {
        WriteError::Io(e)
    }
}

impl From<RpackError> for WriteError {
    fn from(e: RpackError) -> Self {
        WriteError::Model(e)
    }
}

/// Writes `model` to `out_path` as a `.rpack` file: header placeholder,
/// then each section streamed with 8-byte alignment padding while computing
/// its crc32, then the finished header + section table rewritten at offset 0.
pub fn write_rpack(
    model: &RegionGraphModel,
    meta: &PackMeta,
    out_path: impl AsRef<Path>,
) -> Result<(), WriteError> {
    model.validate()?;

    let name_bytes = meta.region_name.as_bytes();
    if name_bytes.len() > REGION_NAME_LEN {
        return Err(WriteError::RegionNameTooLong {
            max: REGION_NAME_LEN,
            got: name_bytes.len(),
        });
    }

    const SECTION_COUNT: usize = 6;
    let header_size = size_of::<HeaderFixed>() as u64;
    let table_len = SECTION_COUNT as u64 * size_of::<SectionEntry>() as u64;

    let file = File::create(out_path)?;
    let mut writer = BufWriter::new(file);

    // Header + section table placeholder, rewritten once real offsets/crcs are known.
    writer.write_all(&vec![0u8; (header_size + table_len) as usize])?;
    let mut pos = header_size + table_len;

    const ZERO: [u8; 8] = [0u8; 8];

    fn write_section(
        writer: &mut BufWriter<File>,
        pos: &mut u64,
        id: u32,
        bytes: &[u8],
    ) -> Result<SectionEntry, WriteError> {
        let pad = alignment_padding(*pos);
        if pad > 0 {
            writer.write_all(&ZERO[..pad as usize])?;
            *pos += pad;
        }
        let offset = *pos;
        writer.write_all(bytes)?;
        *pos += bytes.len() as u64;
        Ok(SectionEntry {
            section_id: id,
            _pad: 0,
            offset,
            len_bytes: bytes.len() as u64,
            crc32: crc32fast::hash(bytes),
            _pad2: 0,
        })
    }

    let mut entries = Vec::with_capacity(SECTION_COUNT);
    entries.push(write_section(
        &mut writer,
        &mut pos,
        SECTION_NODES,
        bytemuck::cast_slice::<NodeRecord, u8>(&model.nodes),
    )?);
    entries.push(write_section(
        &mut writer,
        &mut pos,
        SECTION_CSR,
        bytemuck::cast_slice::<u32, u8>(&model.csr_first_edge),
    )?);
    entries.push(write_section(
        &mut writer,
        &mut pos,
        SECTION_EDGES_HOT,
        bytemuck::cast_slice::<EdgeHot, u8>(&model.edges),
    )?);
    entries.push(write_section(
        &mut writer,
        &mut pos,
        SECTION_CH_ORDER,
        bytemuck::cast_slice::<u32, u8>(&model.ch_order),
    )?);
    entries.push(write_section(
        &mut writer,
        &mut pos,
        SECTION_GEOMETRY,
        bytemuck::cast_slice::<GeomVertex, u8>(&model.geometry),
    )?);

    // SNAP_GRID is header + two arrays concatenated into one section payload.
    {
        let grid = &model.snap_grid;
        let grid_header = SnapGridHeader {
            min_lat: grid.min_lat,
            min_lon: grid.min_lon,
            cell_size_deg: grid.cell_size_deg,
            n_rows: grid.n_rows,
            n_cols: grid.n_cols,
            _pad: 0,
        };
        let header_bytes = bytemuck::bytes_of(&grid_header);
        let cell_offsets_bytes: &[u8] = bytemuck::cast_slice(&grid.cell_offsets);
        let node_ids_bytes: &[u8] = bytemuck::cast_slice(&grid.node_ids);

        let pad = alignment_padding(pos);
        if pad > 0 {
            writer.write_all(&ZERO[..pad as usize])?;
            pos += pad;
        }
        let offset = pos;
        writer.write_all(header_bytes)?;
        writer.write_all(cell_offsets_bytes)?;
        writer.write_all(node_ids_bytes)?;
        let len_bytes =
            (header_bytes.len() + cell_offsets_bytes.len() + node_ids_bytes.len()) as u64;

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(header_bytes);
        hasher.update(cell_offsets_bytes);
        hasher.update(node_ids_bytes);

        entries.push(SectionEntry {
            section_id: SECTION_SNAP_GRID,
            _pad: 0,
            offset,
            len_bytes,
            crc32: hasher.finalize(),
            _pad2: 0,
        });
    }

    let mut region_name = [0u8; REGION_NAME_LEN];
    region_name[..name_bytes.len()].copy_from_slice(name_bytes);
    let header = HeaderFixed {
        magic: packs::MAGIC,
        format_major: packs::FORMAT_MAJOR,
        format_minor: packs::FORMAT_MINOR,
        osm_snapshot_epoch: meta.osm_snapshot_epoch,
        region_id: meta.region_id,
        region_name,
        section_count: entries.len() as u32,
    };

    writer.flush()?;
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(bytemuck::bytes_of(&header))?;
    for entry in &entries {
        writer.write_all(bytemuck::bytes_of(entry))?;
    }
    writer.flush()?;

    Ok(())
}
