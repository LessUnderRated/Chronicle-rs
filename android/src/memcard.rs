//! Saves from a PlayStation 2 memory card: the game's own `darkcloudN` files read out of a card
//! image and put where the port keeps its saves (docs/PC.md, "Saves and host files": `save.dat` is
//! that file unchanged, in `saves/<N>/`).
//!
//! Read: a raw card image (`.ps2`, `.mcd`, `.bin`: PCSX2's and AetherSX2's cards, or a real card
//! dumped with mcpd), with or without the 16 spare bytes of ECC after each page, and a uLaunchELF
//! save export (`.psu`). The card's file system is the one ps2sdk's mcman writes: a superblock in
//! page 0, a two-level FAT reached through its indirect cluster list, and 512-byte directory entries.

use std::fs;
use std::path::{Path, PathBuf};

/// A save as the game writes it: the CSaveData, its version string, and a checksum byte for every
/// 64 bytes of the CSaveData (port/src/memorycardaccess.cpp).
pub const IMAGE_SIZE: usize = 0x136A7;
const SAVE_DATA_SIZE: usize = 0x131C0;
const VERSION_SIZE: usize = 0x20;
/// The version the port's game (PAL's code) writes and reads.
pub const PAL_VERSION: &str = "darkcloudVer1.9";

const MAGIC: &[u8] = b"Sony PS2 Memory Card Format ";
const ENTRY: usize = 512;
const MODE_EXISTS: u16 = 0x8000;
const MODE_DIRECTORY: u16 = 0x0020;
const MODE_FILE: u16 = 0x0010;
const CHAIN_END: u32 = 0x7FFF_FFFF;

#[derive(Debug, Clone, PartialEq)]
pub struct CardFile {
    pub directory: String,
    pub name: String,
    pub data: Vec<u8>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn name_at(entry: &[u8]) -> String {
    let field = &entry[0x40..0x60];
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

struct Entry {
    mode: u16,
    length: u32,
    cluster: u32,
    name: String,
}

fn entry(bytes: &[u8]) -> Option<Entry> {
    let bytes = bytes.get(..ENTRY)?;
    Some(Entry {
        mode: u16_at(bytes, 0)?,
        length: u32_at(bytes, 4)?,
        cluster: u32_at(bytes, 0x10)?,
        name: name_at(bytes),
    })
}

/// Every file of every directory in the card's root, from a card image or a save export.
pub fn read_card(bytes: &[u8]) -> Result<Vec<CardFile>, String> {
    if bytes.starts_with(MAGIC) {
        return Card::new(bytes)?.files();
    }
    if bytes.starts_with(b"Ps2PowerSave") {
        return Err("Action Replay MAX saves (.max) are compressed and not read yet: export the save as .psu \
                    (uLaunchELF, mymc) or choose the whole card image (.ps2)"
            .into());
    }
    if let Some(files) = read_psu(bytes) {
        return Ok(files);
    }
    Err("this is not a PS2 memory card image (.ps2, .mcd, .bin) or a .psu save export".into())
}

struct Card<'a> {
    bytes: &'a [u8],
    page_len: usize,
    page_stride: usize,
    pages_per_cluster: usize,
    clusters: u32,
    alloc_offset: u32,
    root: u32,
    ifc: Vec<u32>,
}

impl<'a> Card<'a> {
    fn new(bytes: &'a [u8]) -> Result<Card<'a>, String> {
        let bad = || "the card's superblock is damaged".to_string();
        let page_len = u16_at(bytes, 0x28).ok_or_else(bad)? as usize;
        let pages_per_cluster = u16_at(bytes, 0x2A).ok_or_else(bad)? as usize;
        let clusters = u32_at(bytes, 0x30).ok_or_else(bad)?;
        let alloc_offset = u32_at(bytes, 0x34).ok_or_else(bad)?;
        let root = u32_at(bytes, 0x3C).ok_or_else(bad)?;
        if !(page_len == 512 || page_len == 1024)
            || pages_per_cluster == 0
            || pages_per_cluster > 16
            || clusters == 0
        {
            return Err(bad());
        }
        let ifc = (0..32)
            .map(|i| u32_at(bytes, 0x50 + 4 * i).unwrap_or(0))
            .collect();
        // A raw dump keeps each page's ECC after it; a plain image does not.
        let pages = clusters as usize * pages_per_cluster;
        let page_stride = if bytes.len() >= pages * (page_len + 16) {
            page_len + 16
        } else {
            page_len
        };
        if bytes.len() < pages * page_stride {
            return Err(format!(
                "the card image is cut short: {} bytes where its superblock needs {}",
                bytes.len(),
                pages * page_stride
            ));
        }
        Ok(Card {
            bytes,
            page_len,
            page_stride,
            pages_per_cluster,
            clusters,
            alloc_offset,
            root,
            ifc,
        })
    }

    fn cluster_size(&self) -> usize {
        self.page_len * self.pages_per_cluster
    }

    /// An absolute cluster's bytes, its pages without their ECC.
    fn cluster(&self, absolute: u32) -> Result<Vec<u8>, String> {
        if absolute >= self.clusters {
            return Err(format!("the card names cluster {absolute}, past its end"));
        }
        let mut out = Vec::with_capacity(self.cluster_size());
        for page in 0..self.pages_per_cluster {
            let start = (absolute as usize * self.pages_per_cluster + page) * self.page_stride;
            out.extend_from_slice(&self.bytes[start..start + self.page_len]);
        }
        Ok(out)
    }

    /// The FAT's entry for a cluster counted from the allocatable area.
    fn fat(&self, relative: u32) -> Result<u32, String> {
        let per = (self.cluster_size() / 4) as u32;
        let fat_cluster = relative / per;
        let indirect = self
            .ifc
            .get((fat_cluster / per) as usize)
            .copied()
            .ok_or("the card's FAT is too large")?;
        let indirect = self.cluster(indirect)?;
        let table = u32_at(&indirect, ((fat_cluster % per) * 4) as usize)
            .ok_or("the card's FAT is damaged")?;
        let table = self.cluster(table)?;
        u32_at(&table, ((relative % per) * 4) as usize)
            .ok_or_else(|| "the card's FAT is damaged".into())
    }

    /// `length` bytes from the chain that starts at `first`.
    fn read_chain(&self, first: u32, length: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(length);
        let mut cluster = first;
        let mut steps = 0;
        while out.len() < length {
            out.extend_from_slice(&self.cluster(self.alloc_offset + cluster)?);
            steps += 1;
            if steps > self.clusters {
                return Err("the card's FAT has a loop".into());
            }
            let next = self.fat(cluster)?;
            if next & 0x8000_0000 == 0 {
                if out.len() < length {
                    return Err("a file on the card runs into free space".into());
                }
                break;
            }
            let next = next & 0x7FFF_FFFF;
            if next == CHAIN_END {
                break;
            }
            cluster = next;
        }
        if out.len() < length {
            return Err("a file on the card ends before its length".into());
        }
        out.truncate(length);
        Ok(out)
    }

    /// A directory's entries: its first, ".", gives their number.
    fn directory(&self, cluster: u32) -> Result<Vec<Entry>, String> {
        let first = self.read_chain(cluster, ENTRY)?;
        let count = entry(&first)
            .ok_or("a directory on the card is damaged")?
            .length as usize;
        if count == 0 || count > 4096 {
            return Err("a directory on the card is damaged".into());
        }
        let bytes = self.read_chain(cluster, count * ENTRY)?;
        Ok(bytes.chunks(ENTRY).filter_map(entry).collect())
    }

    fn files(&self) -> Result<Vec<CardFile>, String> {
        let mut files = Vec::new();
        for dir in self.directory(self.root)? {
            if dir.mode & (MODE_EXISTS | MODE_DIRECTORY) != MODE_EXISTS | MODE_DIRECTORY
                || dir.name.starts_with('.')
            {
                continue;
            }
            let Ok(entries) = self.directory(dir.cluster) else {
                continue;
            };
            for file in entries {
                if file.mode & (MODE_EXISTS | MODE_FILE) != MODE_EXISTS | MODE_FILE
                    || file.length > 16 << 20
                {
                    continue;
                }
                if let Ok(data) = self.read_chain(file.cluster, file.length as usize) {
                    files.push(CardFile {
                        directory: dir.name.clone(),
                        name: file.name,
                        data,
                    });
                }
            }
        }
        Ok(files)
    }
}

/// uLaunchELF's export: the directory's entry, whose length counts the entries after it ("." and
/// ".." among them), then each entry, a file's followed by its data padded to 1024 bytes.
fn read_psu(bytes: &[u8]) -> Option<Vec<CardFile>> {
    let dir = entry(bytes)?;
    if dir.mode & (MODE_EXISTS | MODE_DIRECTORY) != MODE_EXISTS | MODE_DIRECTORY
        || dir.name.is_empty()
    {
        return None;
    }
    let mut at = ENTRY;
    let mut files = Vec::new();
    for _ in 0..dir.length.min(4096) {
        let file = entry(bytes.get(at..)?)?;
        at += ENTRY;
        if file.mode & MODE_FILE != 0 {
            let length = file.length as usize;
            let data = bytes.get(at..at + length)?.to_vec();
            at += length.div_ceil(1024) * 1024;
            files.push(CardFile {
                directory: dir.name.clone(),
                name: file.name,
                data,
            });
        }
    }
    Some(files)
}

// ---- Dark Cloud's saves --------------------------------------------------------------------------

/// What became of a save found on the card.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// In `saves/<folder>/save.dat`; `converted_from` names the version string it had when it was not
    /// PAL's, which the port's game reads, so it was given PAL's.
    Imported {
        folder: u32,
        converted_from: Option<String>,
    },
    AlreadyThere,
    Damaged(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub source: String,
    pub outcome: Outcome,
}

fn checksums_hold(image: &[u8]) -> bool {
    let sums = &image[SAVE_DATA_SIZE + VERSION_SIZE..];
    image[..SAVE_DATA_SIZE]
        .chunks(64)
        .zip(sums)
        .all(|(block, &sum)| block.iter().fold(0u8, |total, &b| total.wrapping_add(b)) == sum)
}

fn version_of(image: &[u8]) -> String {
    let field = &image[SAVE_DATA_SIZE..SAVE_DATA_SIZE + VERSION_SIZE];
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The game's save files on the card: `darkcloudN` in a directory the game made (`BESCES-50295dkcloud`
/// and the like), by directory and then N.
pub fn dark_cloud_saves(files: &[CardFile]) -> Vec<&CardFile> {
    let number = |f: &CardFile| {
        f.name
            .strip_prefix("darkcloud")
            .and_then(|n| n.parse::<u32>().ok())
    };
    let mut saves: Vec<&CardFile> = files
        .iter()
        .filter(|f| f.directory.ends_with("dkcloud") && number(f).is_some())
        .collect();
    saves.sort_by_key(|f| (f.directory.clone(), number(f)));
    saves
}

/// The numbered folders under `saves`.
fn folders(saves: &Path) -> Vec<u32> {
    let mut out: Vec<u32> = fs::read_dir(saves)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok()))
        .collect();
    out.sort_unstable();
    out
}

fn existing_images(saves: &Path) -> Vec<Vec<u8>> {
    fs::read_dir(saves)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read(e.path().join("save.dat")).ok())
        .collect()
}

fn write_durably(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary: PathBuf = path.with_extension("dat.import");
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
}

/// Puts every Dark Cloud save on the card into `saves` (the port's `<save>/saves`), each in the
/// lowest numbered folder free; one already there, byte for byte, is left out. Nothing is ever
/// written over.
pub fn import(card: &[u8], saves: &Path) -> Result<Vec<Found>, String> {
    let files = read_card(card)?;
    let found = dark_cloud_saves(&files);
    if found.is_empty() {
        return Err("there is no Dark Cloud save on this card".into());
    }
    let mut taken = folders(saves);
    let mut existing = existing_images(saves);
    let mut results = Vec::new();
    for file in found {
        let source = format!("{}/{}", file.directory, file.name);
        let outcome = (|| {
            if file.data.len() < IMAGE_SIZE {
                return Outcome::Damaged(format!(
                    "{} bytes where a save has {IMAGE_SIZE}",
                    file.data.len()
                ));
            }
            let mut image = file.data[..IMAGE_SIZE].to_vec();
            if !checksums_hold(&image) {
                return Outcome::Damaged("its checksums do not hold".into());
            }
            let version = version_of(&image);
            let converted_from = (version != PAL_VERSION).then(|| version.clone());
            if converted_from.is_some() {
                // The checksums cover the CSaveData alone, so the version string can change.
                let field = &mut image[SAVE_DATA_SIZE..SAVE_DATA_SIZE + VERSION_SIZE];
                field.fill(0);
                field[..PAL_VERSION.len()].copy_from_slice(PAL_VERSION.as_bytes());
            }
            if existing.contains(&image) {
                return Outcome::AlreadyThere;
            }
            let folder = (1..).find(|n| !taken.contains(n)).unwrap();
            if let Err(error) =
                write_durably(&saves.join(folder.to_string()).join("save.dat"), &image)
            {
                return Outcome::Damaged(format!("it could not be written: {error}"));
            }
            taken.push(folder);
            existing.push(image);
            Outcome::Imported {
                folder,
                converted_from,
            }
        })();
        results.push(Found { source, outcome });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_image(version: &str, seed: u8) -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_SIZE];
        for (i, b) in image[..SAVE_DATA_SIZE].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(seed);
        }
        image[SAVE_DATA_SIZE..SAVE_DATA_SIZE + version.len()].copy_from_slice(version.as_bytes());
        let sums: Vec<u8> = image[..SAVE_DATA_SIZE]
            .chunks(64)
            .map(|c| c.iter().fold(0u8, |t, &b| t.wrapping_add(b)))
            .collect();
        image[SAVE_DATA_SIZE + VERSION_SIZE..SAVE_DATA_SIZE + VERSION_SIZE + sums.len()]
            .copy_from_slice(&sums);
        image
    }

    fn dir_entry(mode: u16, length: u32, cluster: u32, name: &str) -> Vec<u8> {
        let mut e = vec![0u8; ENTRY];
        e[0..2].copy_from_slice(&mode.to_le_bytes());
        e[4..8].copy_from_slice(&length.to_le_bytes());
        e[0x10..0x14].copy_from_slice(&cluster.to_le_bytes());
        e[0x40..0x40 + name.len()].copy_from_slice(name.as_bytes());
        e
    }

    /// A card as mcman lays one out: 512-byte pages, 2 a cluster, 256 clusters, the FAT through
    /// indirect cluster 2 and FAT cluster 3, the allocatable area from cluster 8.
    fn card(files: &[(&str, &str, Vec<u8>)], ecc: bool) -> Vec<u8> {
        const CLUSTERS: usize = 256;
        const ALLOC: u32 = 8;
        let cluster_size = 1024;
        let mut clusters = vec![vec![0u8; cluster_size]; CLUSTERS];
        let mut fat = vec![0x7FFF_FFFFu32; 256];
        let mut next_free = 0u32;
        let mut store = |data: &[u8], fat: &mut Vec<u32>, clusters: &mut Vec<Vec<u8>>| -> u32 {
            let first = next_free;
            let count = data.len().div_ceil(cluster_size).max(1) as u32;
            for i in 0..count {
                let rel = first + i;
                let chunk = &data[(i as usize * cluster_size).min(data.len())
                    ..((i as usize + 1) * cluster_size).min(data.len())];
                clusters[(ALLOC + rel) as usize][..chunk.len()].copy_from_slice(chunk);
                fat[rel as usize] = if i + 1 == count {
                    0xFFFF_FFFF
                } else {
                    0x8000_0000 | (rel + 1)
                };
            }
            next_free += count;
            first
        };
        let mut dirs: Vec<&str> = files.iter().map(|f| f.0).collect();
        dirs.dedup();
        // The root first, so it starts at cluster 0; its entries are filled once the rest is placed.
        let root_entries = 2 + dirs.len();
        let root = store(&vec![0u8; root_entries * ENTRY], &mut fat, &mut clusters);
        let mut root_bytes = [
            dir_entry(0x8427, root_entries as u32, 0, "."),
            dir_entry(0x8427, 0, 0, ".."),
        ]
        .concat();
        for dir in dirs {
            let mine: Vec<&(&str, &str, Vec<u8>)> = files.iter().filter(|f| f.0 == dir).collect();
            let count = 2 + mine.len();
            let at = store(&vec![0u8; count * ENTRY], &mut fat, &mut clusters);
            let mut bytes = [
                dir_entry(0x8427, count as u32, at, "."),
                dir_entry(0x8427, 0, 0, ".."),
            ]
            .concat();
            for (_, name, data) in mine {
                let first = store(data, &mut fat, &mut clusters);
                bytes.extend(dir_entry(0x8497, data.len() as u32, first, name));
            }
            for (i, chunk) in bytes.chunks(cluster_size).enumerate() {
                clusters[(ALLOC + at) as usize + i][..chunk.len()].copy_from_slice(chunk);
            }
            root_bytes.extend(dir_entry(0x8427, count as u32, at, dir));
        }
        for (i, chunk) in root_bytes.chunks(cluster_size).enumerate() {
            clusters[(ALLOC + root) as usize + i][..chunk.len()].copy_from_slice(chunk);
        }
        clusters[2][0..4].copy_from_slice(&3u32.to_le_bytes());
        for (i, e) in fat.iter().enumerate() {
            clusters[3][i * 4..i * 4 + 4].copy_from_slice(&e.to_le_bytes());
        }
        let sb = &mut clusters[0];
        sb[..MAGIC.len()].copy_from_slice(MAGIC);
        sb[0x28..0x2A].copy_from_slice(&512u16.to_le_bytes());
        sb[0x2A..0x2C].copy_from_slice(&2u16.to_le_bytes());
        sb[0x2C..0x2E].copy_from_slice(&16u16.to_le_bytes());
        sb[0x30..0x34].copy_from_slice(&(CLUSTERS as u32).to_le_bytes());
        sb[0x34..0x38].copy_from_slice(&ALLOC.to_le_bytes());
        sb[0x3C..0x40].copy_from_slice(&root.to_le_bytes());
        sb[0x50..0x54].copy_from_slice(&2u32.to_le_bytes());
        let mut image = Vec::new();
        for cluster in clusters {
            for page in cluster.chunks(512) {
                image.extend_from_slice(page);
                if ecc {
                    image.extend_from_slice(&[0xEE; 16]);
                }
            }
        }
        image
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chronicle_mc_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_a_card_with_and_without_ecc() {
        let pal = save_image(PAL_VERSION, 1);
        let other = vec![7u8; 3000];
        for ecc in [false, true] {
            let image = card(
                &[
                    ("BESCES-50295dkcloud", "darkcloud0", pal.clone()),
                    ("BESCES-50295dkcloud", "icon.sys", vec![1; 964]),
                    ("BASLUS-20000other", "data", other.clone()),
                ],
                ecc,
            );
            let files = read_card(&image).unwrap();
            assert_eq!(files.len(), 3, "ecc {ecc}");
            assert_eq!(files[0].data, pal);
            assert_eq!(files[2].data, other);
            let saves = dark_cloud_saves(&files);
            assert_eq!(saves.len(), 1);
            assert_eq!(saves[0].name, "darkcloud0");
        }
    }

    #[test]
    fn imports_into_free_folders_and_converts_other_regions() {
        let saves = temp("import");
        fs::create_dir_all(saves.join("1")).unwrap();
        fs::write(saves.join("1/save.dat"), save_image(PAL_VERSION, 9)).unwrap();
        fs::create_dir_all(saves.join("autosave")).unwrap();
        let us = save_image("darkcloudVer1.02", 2);
        let image = card(
            &[
                ("BASCUS-97111dkcloud", "darkcloud0", us.clone()),
                (
                    "BASCUS-97111dkcloud",
                    "darkcloud3",
                    save_image(PAL_VERSION, 3),
                ),
            ],
            true,
        );
        let found = import(&image, &saves).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].outcome,
            Outcome::Imported {
                folder: 2,
                converted_from: Some("darkcloudVer1.02".into())
            }
        );
        assert_eq!(
            found[1].outcome,
            Outcome::Imported {
                folder: 3,
                converted_from: None
            }
        );
        let written = fs::read(saves.join("2/save.dat")).unwrap();
        assert_eq!(version_of(&written), PAL_VERSION);
        assert!(checksums_hold(&written));
        assert_eq!(written[..SAVE_DATA_SIZE], us[..SAVE_DATA_SIZE]);

        // The same card again brings nothing new.
        let again = import(&image, &saves).unwrap();
        assert!(again.iter().all(|f| f.outcome == Outcome::AlreadyThere));
        assert_eq!(folders(&saves), vec![1, 2, 3]);
        let _ = fs::remove_dir_all(&saves);
    }

    #[test]
    fn turns_down_damaged_saves() {
        let saves = temp("damaged");
        let mut bad = save_image(PAL_VERSION, 4);
        bad[100] ^= 0xFF;
        let image = card(&[("BESCES-50295dkcloud", "darkcloud1", bad)], false);
        let found = import(&image, &saves).unwrap();
        assert!(matches!(found[0].outcome, Outcome::Damaged(_)));
        assert!(folders(&saves).is_empty());
        let _ = fs::remove_dir_all(&saves);
    }

    #[test]
    fn reads_a_psu_export() {
        let pal = save_image(PAL_VERSION, 5);
        let mut psu = dir_entry(0x8427, 3, 0, "BESCES-50295dkcloud");
        psu.extend(dir_entry(0x8427, 0, 0, "."));
        psu.extend(dir_entry(0x8427, 0, 0, ".."));
        psu.extend(dir_entry(0x8497, pal.len() as u32, 0, "darkcloud2"));
        psu.extend(&pal);
        psu.resize(psu.len().div_ceil(1024) * 1024, 0);
        let files = read_card(&psu).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "darkcloud2");
        assert_eq!(files[0].data, pal);
    }

    #[test]
    fn says_what_it_cannot_read() {
        assert!(read_card(b"Ps2PowerSave....").unwrap_err().contains(".max"));
        assert!(read_card(&[0u8; 2048]).is_err());
        let mut cut = card(
            &[(
                "BESCES-50295dkcloud",
                "darkcloud0",
                save_image(PAL_VERSION, 6),
            )],
            false,
        );
        cut.truncate(cut.len() / 2);
        assert!(read_card(&cut).unwrap_err().contains("cut short"));
    }
}
