//! Drive kinds and the FAT layouts DOS reports for them.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Floppy,
    HardDisk,
    CdRom,
    /// A read-only drive held in memory: Z:, and the built-in Ultrasound
    /// software.
    Virtual,
}

impl DriveKind {
    pub fn is_removable(self) -> bool {
        matches!(self, DriveKind::Floppy | DriveKind::CdRom)
    }

    /// Media descriptor byte as found in the FAT / DPB.
    pub fn media_descriptor(self) -> u8 {
        match self {
            DriveKind::Floppy => 0xF0, // 3.5" 1.44 MB
            _ => 0xF8,                 // fixed disk
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DriveKind::Floppy => "floppy",
            DriveKind::HardDisk => "hdd",
            DriveKind::CdRom => "cdrom",
            DriveKind::Virtual => "virtual",
        }
    }

    /// (sectors per cluster, bytes per sector, total clusters) as reported by
    /// INT 21h AH=1Ch/36h. Hard disks keep the long-standing fake 80 MB and
    /// CD-ROMs report what MSCDEX-style redirectors typically do.
    pub fn geometry(self) -> (u16, u16, u16) {
        match self {
            DriveKind::Floppy => (1, 512, FLOPPY_CLUSTERS),
            DriveKind::HardDisk => (8, 512, 20000),
            DriveKind::CdRom => (1, 2048, 0xFFFF),
            DriveKind::Virtual => (1, 512, 2000),
        }
    }

    /// The FAT file system behind `geometry`: a 1.44 MB diskette's for
    /// floppies, and a plausible one around the cluster count for the rest.
    pub fn layout(self) -> FatLayout {
        let (sectors_per_cluster, bytes_per_sector, clusters) = self.geometry();
        let (root_entries, sectors_per_track, heads, hidden_sectors) = match self {
            DriveKind::Floppy => (224, 18, 2, 0),
            _ => (512, 63, 16, 63),
        };
        let fat_bytes = if clusters < FAT12_MAX_CLUSTERS {
            ((clusters as u32 + 2) * 3).div_ceil(2)
        } else {
            (clusters as u32 + 2) * 2
        };
        let mut layout = FatLayout {
            bytes_per_sector,
            sectors_per_cluster,
            clusters: clusters as u32,
            reserved_sectors: 1,
            fats: 2,
            root_entries,
            sectors_per_fat: fat_bytes.div_ceil(bytes_per_sector as u32),
            sectors_per_track,
            heads,
            hidden_sectors,
            media: self.media_descriptor(),
            sectors: 0,
            fat32: None,
        };
        layout.sectors = layout.first_data_sector() + clusters as u32 * sectors_per_cluster as u32;
        layout
    }
}

/// The free space of a drive as the functions of before FAT32 (INT 21h
/// AH=1Bh, 1Ch and 36h) can tell it: (sectors per cluster, free clusters,
/// bytes per sector, total clusters) in 16 bits and never more than just
/// under 2 GB, which programs keep in signed 32-bit numbers. Bigger
/// clusters make up for fewer of them, as long as a cluster stays under
/// 32 KB (FORMAT divides by it), as MS-DOS 7.1 and DOSBox-X report them.
pub fn old_space(spc: u32, free: u32, bps: u32, total: u32) -> (u16, u16, u16, u16) {
    let mut factor = 1;
    while (total > 0xFFFF || free > 0xFFFF) && spc * factor <= 64 && bps * spc * factor < 0x8000 {
        factor *= 2;
    }
    let spc = spc * factor;
    let under_2gb = 0x7FFF_8000 / bps.max(1) / spc.max(1);
    let fit = |n: u32| (n / factor).min(under_2gb).min(0xFFFF) as u16;
    (spc as u16, fit(free), bps as u16, fit(total))
}

/// Volumes with fewer clusters than this have 12-bit FATs.
const FAT12_MAX_CLUSTERS: u16 = 4085;

/// Where a drive's FAT, root directory and data would lie, as DOS reports
/// it in the drive parameter block and the BIOS parameter block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FatLayout {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u16,
    pub clusters: u32,
    pub reserved_sectors: u16,
    pub fats: u16,
    pub root_entries: u16,
    pub sectors_per_fat: u32,
    pub sectors_per_track: u16,
    pub heads: u16,
    pub hidden_sectors: u32,
    pub media: u8,
    /// Sectors in the volume, which may run on past the last cluster.
    pub sectors: u32,
    /// A FAT32 volume's own fields.
    pub fat32: Option<Fat32Layout>,
}

/// The fields of a FAT32 volume's BPB that FAT12 and FAT16 don't have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fat32Layout {
    pub root_cluster: u32,
    pub fsinfo: u16,
    pub backup_boot: u16,
    pub ext_flags: u16,
}

impl FatLayout {
    /// The first sector of the root directory: FAT32's first data sector,
    /// its root directory being a chain of clusters.
    pub fn first_dir_sector(&self) -> u32 {
        self.reserved_sectors as u32 + self.fats as u32 * self.sectors_per_fat
    }

    pub fn first_data_sector(&self) -> u32 {
        let root_sectors = (self.root_entries as u32 * 32).div_ceil(self.bytes_per_sector as u32);
        self.first_dir_sector() + root_sectors
    }

    pub fn total_sectors(&self) -> u32 {
        self.sectors
    }

    pub fn cylinders(&self) -> u16 {
        let per_cylinder = self.sectors_per_track as u32 * self.heads as u32;
        (self.hidden_sectors as u64 + self.total_sectors() as u64).div_ceil(per_cylinder.max(1) as u64).min(u16::MAX as u64) as u16
    }

    /// "FAT12   ", "FAT16   " or "FAT32   ", as the boot sector names it.
    pub fn fs_type(&self) -> &'static [u8; 8] {
        match (self.fat32, self.clusters) {
            (Some(_), _) => b"FAT32   ",
            (None, n) if n < FAT12_MAX_CLUSTERS as u32 => b"FAT12   ",
            _ => b"FAT16   ",
        }
    }

    /// The DOS 4 BIOS parameter block, as a boot sector has it from offset
    /// 0Bh and IOCTL 440Dh/0860h returns it. FAT32's has no FAT size here.
    pub fn bpb(&self) -> [u8; 31] {
        let mut bpb = [0u8; 31];
        let total = self.total_sectors();
        let small_total = if total > 0xFFFF || self.fat32.is_some() { 0 } else { total as u16 };
        let small_fat = if self.fat32.is_some() { 0 } else { self.sectors_per_fat.min(0xFFFF) as u16 };
        bpb[0x00..0x02].copy_from_slice(&self.bytes_per_sector.to_le_bytes());
        bpb[0x02] = self.sectors_per_cluster as u8;
        bpb[0x03..0x05].copy_from_slice(&self.reserved_sectors.to_le_bytes());
        bpb[0x05] = self.fats as u8;
        bpb[0x06..0x08].copy_from_slice(&self.root_entries.to_le_bytes());
        bpb[0x08..0x0A].copy_from_slice(&small_total.to_le_bytes());
        bpb[0x0A] = self.media;
        bpb[0x0B..0x0D].copy_from_slice(&small_fat.to_le_bytes());
        bpb[0x0D..0x0F].copy_from_slice(&self.sectors_per_track.to_le_bytes());
        bpb[0x0F..0x11].copy_from_slice(&self.heads.to_le_bytes());
        bpb[0x11..0x15].copy_from_slice(&self.hidden_sectors.to_le_bytes());
        if small_total == 0 {
            bpb[0x15..0x19].copy_from_slice(&total.to_le_bytes());
        }
        bpb
    }
}
