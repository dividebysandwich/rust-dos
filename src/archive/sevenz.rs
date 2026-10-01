//! 7z archives: their files and folders from the header, and a file's
//! contents decoded with the others of its block, as a solid archive
//! can only be read from the start of a block.

use std::io::{Read, Seek};
use std::time::SystemTime;

use sevenz_rust2::{Archive, BlockDecoder, Password};

/// A file or folder in a 7z archive.
pub struct SevenZEntry {
    /// Its path, with '/' between the parts.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The block its contents are in; None for folders and empty files.
    pub block: Option<usize>,
}

/// The archive's header, which says where everything is.
pub struct SevenZ(Archive);

impl SevenZ {
    pub fn read<F: Read + Seek>(file: &mut F) -> Result<SevenZ, String> {
        Archive::read(file, &Password::empty()).map(SevenZ).map_err(|e| e.to_string())
    }

    pub fn entries(&self) -> Vec<SevenZEntry> {
        let archive = &self.0;
        archive
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_anti_item())
            .map(|(i, f)| SevenZEntry {
                name: f.name().replace('\\', "/").trim_end_matches('/').to_string(),
                is_dir: f.is_directory(),
                size: f.size(),
                modified: f.has_last_modified_date.then(|| f.last_modified_date().into()),
                block: archive.stream_map.file_block_index.get(i).copied().flatten(),
            })
            .collect()
    }

    /// The files of `block`, by their index in `entries`, with their
    /// contents.
    pub fn read_block<F: Read + Seek>(&self, file: &mut F, block: usize) -> Result<Vec<(usize, Vec<u8>)>, String> {
        let archive = &self.0;
        let password = Password::empty();
        let first = *archive.stream_map.block_first_file_index.get(block).ok_or("no such block")?;
        // The entries' indices leave out anti-items, as `entries` does.
        let index = |file: usize| archive.files[..file].iter().filter(|f| !f.is_anti_item()).count();
        let mut files = Vec::new();
        let mut at = first;
        BlockDecoder::new(1, block, archive, &password, file)
            .for_each_entries(&mut |entry, reader| {
                let mut data = Vec::with_capacity(entry.size() as usize);
                reader.read_to_end(&mut data)?;
                if !entry.is_anti_item() {
                    files.push((index(at), data));
                }
                at += 1;
                Ok(true)
            })
            .map_err(|e| e.to_string())?;
        Ok(files)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, SourceReader};

    /// A solid 7z archive of `files` (name, contents), and a folder.
    pub fn sevenz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ArchiveWriter::new(std::io::Cursor::new(Vec::new())).unwrap();
        writer.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory("GAME"), None).unwrap();
        let entries = files.iter().map(|(name, _)| ArchiveEntry::new_file(name)).collect();
        let readers = files.iter().map(|(_, data)| SourceReader::new(*data)).collect();
        writer.push_archive_entries(entries, readers).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_solid_block_is_read_whole() {
        let data = sevenz(&[("GAME/A.TXT", b"first"), ("GAME/B.TXT", &[9; 3000])]);
        let mut file = std::io::Cursor::new(data);
        let archive = SevenZ::read(&mut file).unwrap();
        let entries = archive.entries();
        let names: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("GAME", true), ("GAME/A.TXT", false), ("GAME/B.TXT", false)]);
        let block = entries[1].block.unwrap();
        assert_eq!(entries[2].block, Some(block));
        let files = archive.read_block(&mut file, block).unwrap();
        assert_eq!(files, [(1, b"first".to_vec()), (2, vec![9; 3000])]);
    }
}
