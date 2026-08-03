//! Filesystem-free floppy and disk-image creation for emulator use.
//!
//! The builders in this module use only `alloc`. Callers supply file names and
//! byte slices, and receive a complete deterministic image as a `Vec<u8>`.

use alloc::{format, string::String, vec, vec::Vec};
use core::fmt;

/// A supported emulator disk-image layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiskFormat {
    /// Mackapar M35FD high-density, double-sided media: 448 1024-byte sectors.
    /// The sectors contain a FAT12 volume so named support files can be stored.
    M35Fd,
    /// The same M35FD media with every 16-bit word stored most-significant byte first.
    M35FdBigEndian,
    /// PC-compatible 720 KiB FAT12 floppy used by DOS and CP/M through IS-DOS.
    Fat12_720K,
    /// PC-compatible 1.44 MiB FAT12 floppy used by DOS and MOS/FatFS.
    Fat12_1440K,
    /// Standard 35-track Commodore 1541 disk image.
    Commodore1541,
}

impl DiskFormat {
    /// Parse a format or platform preset name used by the CLI.
    pub fn from_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("m35fd") || name.eq_ignore_ascii_case("dcpu") {
            Some(Self::M35Fd)
        } else if name.eq_ignore_ascii_case("m35fd-be") || name.eq_ignore_ascii_case("dcpu-be") {
            Some(Self::M35FdBigEndian)
        } else if name.eq_ignore_ascii_case("fat12-720")
            || name.eq_ignore_ascii_case("fat12-720k")
            || name.eq_ignore_ascii_case("cpm")
        {
            Some(Self::Fat12_720K)
        } else if name.eq_ignore_ascii_case("fat12-1440")
            || name.eq_ignore_ascii_case("fat12-1440k")
            || name.eq_ignore_ascii_case("dos")
            || name.eq_ignore_ascii_case("mos")
        {
            Some(Self::Fat12_1440K)
        } else if name.eq_ignore_ascii_case("d64") || name.eq_ignore_ascii_case("c64") {
            Some(Self::Commodore1541)
        } else {
            None
        }
    }

    /// Exact byte length produced for this format.
    pub const fn image_size(self) -> usize {
        match self {
            Self::M35Fd | Self::M35FdBigEndian => 448 * 1024,
            Self::Fat12_720K => 1_440 * 512,
            Self::Fat12_1440K => 2_880 * 512,
            Self::Commodore1541 => 174_848,
        }
    }
}

/// Commodore DOS file type used when a file is placed in a D64 image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum C64FileType {
    /// Use PRG for names ending in `.prg`, otherwise use SEQ.
    #[default]
    Auto,
    Program,
    Sequential,
}

/// One named file to place in a disk image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskFile<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
    pub c64_file_type: C64FileType,
}

impl<'a> DiskFile<'a> {
    pub const fn new(name: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            name,
            bytes,
            c64_file_type: C64FileType::Auto,
        }
    }

    pub const fn with_c64_file_type(mut self, file_type: C64FileType) -> Self {
        self.c64_file_type = file_type;
        self
    }
}

/// Input for [`create_disk_image`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskRequest<'a> {
    pub format: DiskFormat,
    pub label: &'a str,
    pub files: &'a [DiskFile<'a>],
}

impl<'a> DiskRequest<'a> {
    pub const fn new(format: DiskFormat, label: &'a str, files: &'a [DiskFile<'a>]) -> Self {
        Self {
            format,
            label,
            files,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskError {
    pub message: String,
}

impl DiskError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DiskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiskError {}

/// Create a complete disk image without host filesystem access.
///
/// FAT images use uppercase 8.3 names and a root-only directory. D64 images use
/// uppercase ASCII/PETSCII names, PRG or SEQ files, and standard 1541 chains.
/// The M35FD image follows the official 224-kword high-density geometry and
/// stores a FAT12 volume in its 1024-byte sectors.
pub fn create_disk_image(request: &DiskRequest<'_>) -> Result<Vec<u8>, DiskError> {
    match request.format {
        DiskFormat::M35Fd => create_fat12_image(Fat12Geometry::M35_FD, request),
        DiskFormat::M35FdBigEndian => {
            let mut image = create_fat12_image(Fat12Geometry::M35_FD, request)?;
            for word in image.as_chunks_mut::<2>().0 {
                word.swap(0, 1);
            }
            Ok(image)
        }
        DiskFormat::Fat12_720K => create_fat12_image(Fat12Geometry::FLOPPY_720K, request),
        DiskFormat::Fat12_1440K => create_fat12_image(Fat12Geometry::FLOPPY_1440K, request),
        DiskFormat::Commodore1541 => create_d64_image(request),
    }
}

#[derive(Clone, Copy)]
struct Fat12Geometry {
    bytes_per_sector: usize,
    total_sectors: usize,
    sectors_per_cluster: usize,
    sectors_per_fat: usize,
    root_entries: usize,
    sectors_per_track: u16,
    heads: u16,
    media: u8,
}

impl Fat12Geometry {
    const M35_FD: Self = Self {
        bytes_per_sector: 1024,
        total_sectors: 448,
        sectors_per_cluster: 1,
        sectors_per_fat: 1,
        root_entries: 112,
        sectors_per_track: 7,
        heads: 2,
        media: 0xf9,
    };

    const FLOPPY_720K: Self = Self {
        bytes_per_sector: 512,
        total_sectors: 1_440,
        sectors_per_cluster: 2,
        sectors_per_fat: 3,
        root_entries: 112,
        sectors_per_track: 9,
        heads: 2,
        media: 0xf9,
    };

    const FLOPPY_1440K: Self = Self {
        bytes_per_sector: 512,
        total_sectors: 2_880,
        sectors_per_cluster: 1,
        sectors_per_fat: 9,
        root_entries: 224,
        sectors_per_track: 18,
        heads: 2,
        media: 0xf0,
    };

    const fn root_sectors(self) -> usize {
        (self.root_entries * 32).div_ceil(self.bytes_per_sector)
    }

    const fn data_start_sector(self) -> usize {
        1 + 2 * self.sectors_per_fat + self.root_sectors()
    }

    const fn data_clusters(self) -> usize {
        (self.total_sectors - self.data_start_sector()) / self.sectors_per_cluster
    }

    const fn cluster_size(self) -> usize {
        self.bytes_per_sector * self.sectors_per_cluster
    }
}

fn create_fat12_image(
    geometry: Fat12Geometry,
    request: &DiskRequest<'_>,
) -> Result<Vec<u8>, DiskError> {
    let label = fat_label(request.label)?;
    let mut names = Vec::with_capacity(request.files.len());
    let mut required_clusters = 0usize;
    for file in request.files {
        let name = fat_name(file.name)?;
        if names.contains(&name) {
            return Err(DiskError::new(format!(
                "duplicate FAT 8.3 file name `{}`",
                file.name
            )));
        }
        names.push(name);
        required_clusters = required_clusters
            .checked_add(file.bytes.len().div_ceil(geometry.cluster_size()))
            .ok_or_else(|| DiskError::new("disk file sizes overflow the host address space"))?;
        if file.bytes.len() > u32::MAX as usize {
            return Err(DiskError::new(format!(
                "file `{}` is too large for a FAT directory entry",
                file.name
            )));
        }
    }

    if request.files.len() + 1 > geometry.root_entries {
        return Err(DiskError::new(format!(
            "{} files plus the volume label exceed the {}-entry FAT root directory",
            request.files.len(),
            geometry.root_entries
        )));
    }
    if required_clusters > geometry.data_clusters() {
        return Err(DiskError::new(format!(
            "files require {required_clusters} FAT clusters but the image has {}",
            geometry.data_clusters()
        )));
    }

    let mut image = vec![0u8; geometry.total_sectors * geometry.bytes_per_sector];
    write_fat_boot_sector(&mut image, geometry, &label);

    let fat_bytes = geometry.sectors_per_fat * geometry.bytes_per_sector;
    let mut fat = vec![0u8; fat_bytes];
    fat[0..3].copy_from_slice(&[geometry.media, 0xff, 0xff]);

    let root_start_sector = 1 + 2 * geometry.sectors_per_fat;
    let root_start = root_start_sector * geometry.bytes_per_sector;
    write_fat_directory_entry(&mut image[root_start..root_start + 32], &label, 0x08, 0, 0);

    let data_start = geometry.data_start_sector() * geometry.bytes_per_sector;
    let mut next_cluster = 2u16;
    for (index, (file, name)) in request.files.iter().zip(names.iter()).enumerate() {
        let cluster_count = file.bytes.len().div_ceil(geometry.cluster_size());
        let first_cluster = if cluster_count == 0 { 0 } else { next_cluster };
        let entry_start = root_start + (index + 1) * 32;
        write_fat_directory_entry(
            &mut image[entry_start..entry_start + 32],
            name,
            0x20,
            first_cluster,
            file.bytes.len() as u32,
        );

        for cluster_index in 0..cluster_count {
            let cluster = first_cluster + cluster_index as u16;
            let next = if cluster_index + 1 == cluster_count {
                0x0fff
            } else {
                cluster + 1
            };
            set_fat12_entry(&mut fat, cluster, next);

            let source_start = cluster_index * geometry.cluster_size();
            let source_end = (source_start + geometry.cluster_size()).min(file.bytes.len());
            let destination = data_start + usize::from(cluster - 2) * geometry.cluster_size();
            image[destination..destination + source_end - source_start]
                .copy_from_slice(&file.bytes[source_start..source_end]);
        }
        next_cluster += cluster_count as u16;
    }

    let first_fat = geometry.bytes_per_sector;
    let second_fat = first_fat + fat_bytes;
    image[first_fat..second_fat].copy_from_slice(&fat);
    image[second_fat..second_fat + fat_bytes].copy_from_slice(&fat);
    Ok(image)
}

fn write_fat_boot_sector(image: &mut [u8], geometry: Fat12Geometry, label: &[u8; 11]) {
    image[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    image[3..11].copy_from_slice(b"EZRAC   ");
    image[11..13].copy_from_slice(&(geometry.bytes_per_sector as u16).to_le_bytes());
    image[13] = geometry.sectors_per_cluster as u8;
    image[14..16].copy_from_slice(&1u16.to_le_bytes());
    image[16] = 2;
    image[17..19].copy_from_slice(&(geometry.root_entries as u16).to_le_bytes());
    image[19..21].copy_from_slice(&(geometry.total_sectors as u16).to_le_bytes());
    image[21] = geometry.media;
    image[22..24].copy_from_slice(&(geometry.sectors_per_fat as u16).to_le_bytes());
    image[24..26].copy_from_slice(&geometry.sectors_per_track.to_le_bytes());
    image[26..28].copy_from_slice(&geometry.heads.to_le_bytes());
    image[36] = 0;
    image[38] = 0x29;
    image[39..43].copy_from_slice(&0x455a_5241u32.to_le_bytes());
    image[43..54].copy_from_slice(label);
    image[54..62].copy_from_slice(b"FAT12   ");
    image[510..512].copy_from_slice(&[0x55, 0xaa]);
}

fn write_fat_directory_entry(
    entry: &mut [u8],
    name: &[u8; 11],
    attributes: u8,
    first_cluster: u16,
    size: u32,
) {
    entry.fill(0);
    entry[0..11].copy_from_slice(name);
    entry[11] = attributes;
    entry[14..16].copy_from_slice(&0u16.to_le_bytes());
    entry[16..18].copy_from_slice(&0x0021u16.to_le_bytes());
    entry[18..20].copy_from_slice(&0x0021u16.to_le_bytes());
    entry[22..24].copy_from_slice(&0u16.to_le_bytes());
    entry[24..26].copy_from_slice(&0x0021u16.to_le_bytes());
    entry[26..28].copy_from_slice(&first_cluster.to_le_bytes());
    entry[28..32].copy_from_slice(&size.to_le_bytes());
}

fn set_fat12_entry(fat: &mut [u8], cluster: u16, value: u16) {
    let cluster = usize::from(cluster);
    let offset = cluster + cluster / 2;
    if cluster & 1 == 0 {
        fat[offset] = value as u8;
        fat[offset + 1] = (fat[offset + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
    } else {
        fat[offset] = (fat[offset] & 0x0f) | ((value << 4) as u8 & 0xf0);
        fat[offset + 1] = (value >> 4) as u8;
    }
}

fn fat_name(name: &str) -> Result<[u8; 11], DiskError> {
    if !name.is_ascii() {
        return Err(DiskError::new(format!(
            "FAT file name `{name}` must contain only ASCII characters"
        )));
    }
    if name.matches('.').count() > 1 {
        return Err(DiskError::new(format!(
            "FAT file name `{name}` must use an 8.3 name with at most one dot"
        )));
    }
    let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
    if stem.is_empty()
        || stem.len() > 8
        || extension.len() > 3
        || (name.contains('.') && extension.is_empty())
    {
        return Err(DiskError::new(format!(
            "FAT file name `{name}` is not a valid 8.3 name"
        )));
    }
    if !stem
        .bytes()
        .chain(extension.bytes())
        .all(valid_fat_name_byte)
    {
        return Err(DiskError::new(format!(
            "FAT file name `{name}` contains a character not allowed in an 8.3 name"
        )));
    }

    let uppercase_stem = stem.to_ascii_uppercase();
    if is_reserved_dos_name(&uppercase_stem) {
        return Err(DiskError::new(format!(
            "FAT file name `{name}` uses a reserved DOS device name"
        )));
    }

    let mut output = [b' '; 11];
    for (slot, byte) in output[..8].iter_mut().zip(stem.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    for (slot, byte) in output[8..].iter_mut().zip(extension.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    Ok(output)
}

fn valid_fat_name_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
        && byte != b'.'
        && !matches!(
            byte,
            b'"' | b'*'
                | b'+'
                | b','
                | b'/'
                | b':'
                | b';'
                | b'<'
                | b'='
                | b'>'
                | b'?'
                | b'['
                | b'\\'
                | b']'
                | b'|'
        )
}

fn is_reserved_dos_name(stem: &str) -> bool {
    matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

fn fat_label(label: &str) -> Result<[u8; 11], DiskError> {
    if label.is_empty() || label.len() > 11 || !label.is_ascii() {
        return Err(DiskError::new(
            "FAT volume label must contain 1 through 11 ASCII characters",
        ));
    }
    if !label.bytes().all(|byte| {
        (byte == b' ' || byte.is_ascii_graphic())
            && !matches!(
                byte,
                b'"' | b'*'
                    | b'+'
                    | b','
                    | b'.'
                    | b'/'
                    | b':'
                    | b';'
                    | b'<'
                    | b'='
                    | b'>'
                    | b'?'
                    | b'['
                    | b'\\'
                    | b']'
                    | b'|'
            )
    }) {
        return Err(DiskError::new(format!(
            "FAT volume label `{label}` contains an unsupported character"
        )));
    }
    let mut output = [b' '; 11];
    for (slot, byte) in output.iter_mut().zip(label.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    Ok(output)
}

const D64_TRACKS: u8 = 35;
const D64_DIRECTORY_TRACK: u8 = 18;
const D64_SECTOR_SIZE: usize = 256;
const D64_SIZE: usize = 174_848;

fn create_d64_image(request: &DiskRequest<'_>) -> Result<Vec<u8>, DiskError> {
    let label = d64_name(request.label, 16, "disk label")?;
    if request.files.len() > 144 {
        return Err(DiskError::new(format!(
            "{} files exceed the 144-entry Commodore 1541 directory",
            request.files.len()
        )));
    }

    let mut names = Vec::with_capacity(request.files.len());
    let mut total_blocks = 0usize;
    for file in request.files {
        let name = d64_name(file.name, 16, "file name")?;
        if names.contains(&name) {
            return Err(DiskError::new(format!(
                "duplicate Commodore file name `{}`",
                file.name
            )));
        }
        names.push(name);
        total_blocks = total_blocks
            .checked_add(file.bytes.len().div_ceil(254).max(1))
            .ok_or_else(|| DiskError::new("disk file sizes overflow the host address space"))?;
    }

    let available_blocks = d64_total_sectors() - usize::from(d64_sectors_on_track(18));
    if total_blocks > available_blocks {
        return Err(DiskError::new(format!(
            "files require {total_blocks} Commodore blocks but the image has {available_blocks} data blocks"
        )));
    }

    let directory_sectors = request.files.len().div_ceil(8).max(1);
    let mut image = vec![0u8; D64_SIZE];
    initialize_d64_bam(&mut image, &label);
    d64_mark_allocated(&mut image, 18, 0);
    for sector in 1..=directory_sectors as u8 {
        d64_mark_allocated(&mut image, 18, sector);
        let offset = d64_offset(18, sector);
        if usize::from(sector) < directory_sectors {
            image[offset] = 18;
            image[offset + 1] = sector + 1;
        } else {
            image[offset] = 0;
            image[offset + 1] = 0xff;
        }
    }

    let mut free_data_sectors = Vec::with_capacity(available_blocks);
    for track in 1..=D64_TRACKS {
        if track == D64_DIRECTORY_TRACK {
            continue;
        }
        for sector in 0..d64_sectors_on_track(track) {
            free_data_sectors.push((track, sector));
        }
    }

    let mut next_data_sector = 0usize;
    for (index, (file, name)) in request.files.iter().zip(names.iter()).enumerate() {
        let block_count = file.bytes.len().div_ceil(254).max(1);
        let first = free_data_sectors[next_data_sector];
        for block_index in 0..block_count {
            let (track, sector) = free_data_sectors[next_data_sector + block_index];
            d64_mark_allocated(&mut image, track, sector);
            let offset = d64_offset(track, sector);
            let source_start = block_index * 254;
            let source_end = (source_start + 254).min(file.bytes.len());
            let byte_count = source_end - source_start;
            if block_index + 1 == block_count {
                image[offset] = 0;
                image[offset + 1] = (byte_count + 1) as u8;
            } else {
                let next = free_data_sectors[next_data_sector + block_index + 1];
                image[offset] = next.0;
                image[offset + 1] = next.1;
            }
            image[offset + 2..offset + 2 + byte_count]
                .copy_from_slice(&file.bytes[source_start..source_end]);
        }
        next_data_sector += block_count;

        let directory_sector = 1 + index / 8;
        let directory_slot = index % 8;
        let entry = d64_offset(18, directory_sector as u8) + directory_slot * 32;
        image[entry + 2] = match resolved_c64_file_type(file) {
            C64FileType::Program => 0x82,
            C64FileType::Sequential => 0x81,
            C64FileType::Auto => unreachable!("auto file type is resolved"),
        };
        image[entry + 3] = first.0;
        image[entry + 4] = first.1;
        image[entry + 5..entry + 21].copy_from_slice(name);
        image[entry + 30..entry + 32].copy_from_slice(&(block_count as u16).to_le_bytes());
    }

    Ok(image)
}

fn resolved_c64_file_type(file: &DiskFile<'_>) -> C64FileType {
    match file.c64_file_type {
        C64FileType::Auto if file.name.to_ascii_lowercase().ends_with(".prg") => {
            C64FileType::Program
        }
        C64FileType::Auto => C64FileType::Sequential,
        file_type => file_type,
    }
}

fn d64_name(name: &str, maximum: usize, kind: &str) -> Result<[u8; 16], DiskError> {
    if name.is_empty() || name.len() > maximum || !name.is_ascii() {
        return Err(DiskError::new(format!(
            "Commodore {kind} must contain 1 through {maximum} ASCII characters"
        )));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return Err(DiskError::new(format!(
            "Commodore {kind} `{name}` contains an unsupported character"
        )));
    }
    let mut output = [0xa0; 16];
    for (slot, byte) in output.iter_mut().zip(name.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    Ok(output)
}

fn initialize_d64_bam(image: &mut [u8], label: &[u8; 16]) {
    let offset = d64_offset(18, 0);
    image[offset] = 18;
    image[offset + 1] = 1;
    image[offset + 2] = 0x41;
    for track in 1..=D64_TRACKS {
        let entry = offset + 4 + usize::from(track - 1) * 4;
        let sector_count = d64_sectors_on_track(track);
        image[entry] = sector_count;
        for sector in 0..sector_count {
            image[entry + 1 + usize::from(sector / 8)] |= 1 << (sector % 8);
        }
    }
    image[offset + 0x90..offset + 0xa0].copy_from_slice(label);
    image[offset + 0xa0..offset + 0xa2].fill(0xa0);
    image[offset + 0xa2..offset + 0xa4].copy_from_slice(b"EZ");
    image[offset + 0xa4] = 0xa0;
    image[offset + 0xa5..offset + 0xa7].copy_from_slice(b"2A");
    image[offset + 0xa7..offset + 0xab].fill(0xa0);
}

fn d64_mark_allocated(image: &mut [u8], track: u8, sector: u8) {
    let bam = d64_offset(18, 0);
    let entry = bam + 4 + usize::from(track - 1) * 4;
    let mask = 1 << (sector % 8);
    let bitmap = entry + 1 + usize::from(sector / 8);
    if image[bitmap] & mask != 0 {
        image[bitmap] &= !mask;
        image[entry] -= 1;
    }
}

const fn d64_sectors_on_track(track: u8) -> u8 {
    match track {
        1..=17 => 21,
        18..=24 => 19,
        25..=30 => 18,
        31..=35 => 17,
        _ => 0,
    }
}

const fn d64_total_sectors() -> usize {
    17 * 21 + 7 * 19 + 6 * 18 + 5 * 17
}

fn d64_offset(track: u8, sector: u8) -> usize {
    let sectors_before = (1..track)
        .map(d64_sectors_on_track)
        .map(usize::from)
        .sum::<usize>();
    (sectors_before + usize::from(sector)) * D64_SECTOR_SIZE
}

#[cfg(test)]
mod tests;
