//! `LTAR` v3 archives (`*.arch05`).
//!
//! ```text
//! 0x00  "LTAR"
//! 0x04  u32 version (3)
//! 0x08  u32 name table size
//! 0x0C  u32 directory count
//! 0x10  u32 file count
//! 0x30  name table (NUL-terminated, 4-byte aligned)
//! then  file entries, 32 bytes each:
//!         u32 name offset, u64 data offset, u64 compressed size, u64 size, u32 compression
//! then  directory entries (unused here; file names are unique per archive)
//! ```
//!
//! Compression 9 stores the file as chunks of `{u32 csize, u32 usize, data}`, each chunk
//! padded so the next header is 4-byte aligned in the archive. A chunk with
//! `csize == usize` is stored raw, otherwise it is a zlib stream.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::{cstr_at, u32_at, u64_at};

const HEADER_SIZE: usize = 0x30;
const ENTRY_SIZE: usize = 32;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub comp_size: u64,
    pub size: u64,
    pub compression: u32,
}

#[derive(Debug)]
pub struct Archive {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut header = [0u8; HEADER_SIZE];
        file.read_exact(&mut header)?;
        ensure!(&header[..4] == b"LTAR", "{}: not an LTAR archive", path.display());
        let version = u32_at(&header, 4)?;
        ensure!(version == 3, "{}: unsupported LTAR version {version}", path.display());
        let name_table_size = u32_at(&header, 8)? as usize;
        let file_count = u32_at(&header, 0x10)? as usize;

        let mut table = vec![0u8; name_table_size + file_count * ENTRY_SIZE];
        file.read_exact(&mut table)?;
        let (names, raw_entries) = table.split_at(name_table_size);

        let mut entries = Vec::with_capacity(file_count);
        for raw in raw_entries.chunks_exact(ENTRY_SIZE) {
            entries.push(Entry {
                name: cstr_at(names, u32_at(raw, 0)? as usize)?,
                offset: u64_at(raw, 4)?,
                comp_size: u64_at(raw, 12)?,
                size: u64_at(raw, 20)?,
                compression: u32_at(raw, 28)?,
            });
        }
        Ok(Self { path, entries })
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    pub fn reader(&self, entry: &Entry) -> Result<EntryReader> {
        EntryReader::new(&self.path, entry)
    }

    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        let mut reader = self.reader(entry)?;
        let mut out = vec![0u8; entry.size as usize];
        reader.read_at(0, &mut out)?;
        Ok(out)
    }
}

struct Chunk {
    /// Offset of this chunk's data in the decompressed file.
    start: u64,
    /// Position of the chunk payload in the archive.
    file_pos: u64,
    comp_size: u32,
    size: u32,
}

/// Random access into one archive entry, decompressing only the chunks touched.
pub struct EntryReader {
    file: File,
    size: u64,
    /// Empty for uncompressed entries.
    chunks: Vec<Chunk>,
    raw_offset: u64,
    cached: Option<(usize, Vec<u8>)>,
}

impl EntryReader {
    fn new(path: &Path, entry: &Entry) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut chunks = Vec::new();
        match entry.compression {
            0 => {}
            9 => {
                let end = entry.offset + entry.comp_size;
                let mut pos = entry.offset;
                let mut start = 0u64;
                let mut header = [0u8; 8];
                while pos < end && start < entry.size {
                    file.seek(SeekFrom::Start(pos))?;
                    file.read_exact(&mut header)?;
                    let comp_size = u32_at(&header, 0)?;
                    let size = u32_at(&header, 4)?;
                    chunks.push(Chunk { start, file_pos: pos + 8, comp_size, size });
                    start += size as u64;
                    pos += 8 + comp_size as u64;
                    pos = pos.next_multiple_of(4);
                }
                ensure!(
                    start == entry.size,
                    "{}: chunks cover {start} of {} bytes",
                    entry.name,
                    entry.size
                );
            }
            other => bail!("{}: unknown compression {other}", entry.name),
        }
        Ok(Self { file, size: entry.size, chunks, raw_offset: entry.offset, cached: None })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn load_chunk(&mut self, index: usize) -> Result<()> {
        if matches!(&self.cached, Some((cached, _)) if *cached == index) {
            return Ok(());
        }
        let chunk = &self.chunks[index];
        let mut raw = vec![0u8; chunk.comp_size as usize];
        self.file.seek(SeekFrom::Start(chunk.file_pos))?;
        self.file.read_exact(&mut raw)?;
        let data = if chunk.comp_size == chunk.size {
            raw
        } else {
            let mut out = Vec::with_capacity(chunk.size as usize);
            flate2::read::ZlibDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
            ensure!(out.len() == chunk.size as usize, "chunk {index} inflated to wrong size");
            out
        };
        self.cached = Some((index, data));
        Ok(())
    }

    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        ensure!(offset + buf.len() as u64 <= self.size, "read past end of entry");
        if self.chunks.is_empty() {
            self.file.seek(SeekFrom::Start(self.raw_offset + offset))?;
            self.file.read_exact(buf)?;
            return Ok(());
        }
        let mut index = self.chunks.partition_point(|c| c.start + c.size as u64 <= offset);
        let mut done = 0;
        while done < buf.len() {
            self.load_chunk(index)?;
            let (_, data) = self.cached.as_ref().unwrap();
            let skip = (offset + done as u64 - self.chunks[index].start) as usize;
            let n = (data.len() - skip).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&data[skip..skip + n]);
            done += n;
            index += 1;
        }
        Ok(())
    }

    pub fn read_vec(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        self.read_at(offset, &mut out)?;
        Ok(out)
    }
}
