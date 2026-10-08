//! `BNDL` v4 resource bundles (`*.embb`, stored inside the archives).
//!
//! ```text
//! 0x00  "BNDL"
//! 0x04  u32 version (4)
//! 0x08  u32 name table size
//! 0x0C  u32 name table size + 8
//! 0x10  u32 0
//! 0x14  u32 sub-bundle count
//! 0x18  u32 record count
//! 0x1C  name table
//! then  8-byte bundle type ("Global\0\0")
//! then  sub-bundle count x u32 name offsets
//! then  records, back to back: u32 name offset, u32 size, data
//! ```
//!
//! Each `.embb` has a sibling `.bndlxml05` XML manifest listing what went into it.

use std::collections::HashMap;

use anyhow::{Result, ensure};

use crate::ltar::EntryReader;
use crate::{cstr_at, normalize_path, u32_at};

const HEADER_SIZE: usize = 0x1C;

#[derive(Debug, Clone)]
pub struct Record {
    /// Path as stored in the bundle (backslashes, original case).
    pub name: String,
    /// Offset of the record data inside the decompressed bundle.
    pub offset: u64,
    pub size: u32,
}

pub struct Bundle {
    reader: EntryReader,
    pub sub_bundles: Vec<String>,
    pub records: Vec<Record>,
    by_name: HashMap<String, usize>,
}

impl Bundle {
    pub fn open(mut reader: EntryReader) -> Result<Self> {
        let header = reader.read_vec(0, HEADER_SIZE)?;
        ensure!(&header[..4] == b"BNDL", "not a BNDL bundle");
        let version = u32_at(&header, 4)?;
        ensure!(version == 4, "unsupported BNDL version {version}");
        let name_table_size = u32_at(&header, 8)? as usize;
        let sub_bundle_count = u32_at(&header, 0x14)? as usize;
        let record_count = u32_at(&header, 0x18)? as usize;

        let names = reader.read_vec(HEADER_SIZE as u64, name_table_size)?;
        let mut pos = (HEADER_SIZE + name_table_size + 8) as u64;

        let raw_subs = reader.read_vec(pos, sub_bundle_count * 4)?;
        let sub_bundles = raw_subs
            .chunks_exact(4)
            .map(|raw| cstr_at(&names, u32_at(raw, 0)? as usize))
            .collect::<Result<Vec<_>>>()?;
        pos += raw_subs.len() as u64;

        let mut records = Vec::with_capacity(record_count);
        let mut by_name = HashMap::with_capacity(record_count);
        let mut head = [0u8; 8];
        for index in 0..record_count {
            reader.read_at(pos, &mut head)?;
            let name = cstr_at(&names, u32_at(&head, 0)? as usize)?;
            let size = u32_at(&head, 4)?;
            by_name.insert(normalize_path(&name), index);
            records.push(Record { name, offset: pos + 8, size });
            pos += 8 + size as u64;
        }
        ensure!(pos <= reader.size(), "records run past the end of the bundle");
        Ok(Self { reader, sub_bundles, records, by_name })
    }

    pub fn find(&self, path: &str) -> Option<&Record> {
        self.by_name.get(&normalize_path(path)).map(|&i| &self.records[i])
    }

    pub fn read(&mut self, record: &Record) -> Result<Vec<u8>> {
        self.reader.read_vec(record.offset, record.size as usize)
    }

    /// First `len` bytes of a record (clamped to its size).
    pub fn peek(&mut self, record: &Record, len: usize) -> Result<Vec<u8>> {
        self.reader.read_vec(record.offset, len.min(record.size as usize))
    }
}

/// Asset names listed in a `.bndlxml05` manifest, with their declared sizes.
pub fn parse_manifest(xml: &[u8]) -> Vec<(String, Option<u64>)> {
    let text = String::from_utf8_lossy(xml);
    let mut out = Vec::new();
    for asset in text.split("<BundleAsset>").skip(1) {
        let Some(name) = tag(asset, "Name") else { continue };
        let size = tag(asset, "Size").and_then(|s| s.parse().ok());
        out.push((name.to_string(), size));
    }
    out
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find("</")? + start;
    Some(text[start..end].trim())
}
