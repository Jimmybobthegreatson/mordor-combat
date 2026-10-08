//! `GADB` v17 game database (`database/game/game.gamedb`).
//!
//! A schema-driven table of typed records. After a 0x2c-byte header and the file's own name:
//!
//! ```text
//! blob            string pool (header u32 @8 bytes); strings are NUL-terminated
//! cells           header u32 @0x1c entries of 15 bytes:
//!                   u32 name hash, u32, u8, u8, u8 kind, u16 count, u16 first value
//! u32, u32        (unused), type count
//! types           24-byte header { u32 name, u32, u32, u32 field count, u32 record count, u32 }
//!                 then `field count` inline structs and `record count` named records:
//!                   [u32 name (records only)] u16 value count, u8 cell count, u32 first cell, u8 flag,
//!                   u32 values[]
//! ```
//!
//! A record's fields are `cells[first_cell .. first_cell + cell_count]`; each cell names (by
//! [`crate::hash::name_hash`]) a field and says how to read `count` of the record's `values`
//! starting at `first value`. Kinds 4..=9 are string offsets into the blob, 11 is a reference to
//! another record (`type index << 20 | item index`; items count the type's inline structs first,
//! then its records), 10 is a reference by name hash.

use anyhow::{Result, anyhow, ensure};

use crate::{cstr_at, u32_at};

/// Field holding the list of material paths in a `_StructuresMaterialSet`.
pub const MATERIAL_PATHS: u32 = 0x610e_3b97;

#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub hash: u32,
    pub kind: u8,
    pub count: u16,
    pub first: u16,
}

#[derive(Debug, Clone)]
struct Item {
    name: Option<u32>,
    first_cell: u32,
    cell_count: u8,
    values: Vec<u32>,
}

#[derive(Debug, Clone)]
struct Type {
    name: u32,
    fields: Vec<Item>,
    records: Vec<Item>,
}

pub struct GameDb {
    data: Vec<u8>,
    blob: usize,
    blob_len: usize,
    cells: Vec<Cell>,
    types: Vec<Type>,
}

/// One record (or inline struct) of the database.
#[derive(Clone, Copy)]
pub struct Entry<'a> {
    db: &'a GameDb,
    item: &'a Item,
}

/// A field's values, with the string offsets already resolved.
#[derive(Debug, Clone)]
pub enum Value {
    Strings(Vec<String>),
    Refs(Vec<u32>),
    Raw { kind: u8, values: Vec<u32> },
}

impl GameDb {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        ensure!(data.len() > 0x30 && &data[..4] == b"GADB", "not a game database");
        ensure!(u32_at(&data, 4)? == 0x11, "unsupported GADB version {}", u32_at(&data, 4)?);
        let blob_len = u32_at(&data, 8)? as usize;
        let cell_count = u32_at(&data, 0x1c)? as usize;
        let name_len = u16::from_le_bytes([data[0x2c], data[0x2d]]) as usize;
        let blob = 0x2e + name_len;
        let cells_at = blob + blob_len;

        let mut cells = Vec::with_capacity(cell_count);
        for i in 0..cell_count {
            let at = cells_at + i * 15;
            let raw = data.get(at..at + 15).ok_or_else(|| anyhow!("cell table truncated"))?;
            cells.push(Cell {
                hash: u32::from_le_bytes(raw[0..4].try_into().unwrap()),
                kind: raw[10] & 0x3f,
                count: u16::from_le_bytes([raw[11], raw[12]]),
                first: u16::from_le_bytes([raw[13], raw[14]]),
            });
        }

        let mut at = cells_at + cell_count * 15;
        let type_count = u32_at(&data, at + 4)? as usize;
        at += 8;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            let name = u32_at(&data, at)?;
            let field_count = u32_at(&data, at + 12)? as usize;
            let record_count = u32_at(&data, at + 16)? as usize;
            at += 24;
            let mut fields = Vec::with_capacity(field_count);
            for _ in 0..field_count {
                fields.push(read_item(&data, &mut at, false)?);
            }
            let mut records = Vec::with_capacity(record_count);
            for _ in 0..record_count {
                records.push(read_item(&data, &mut at, true)?);
            }
            types.push(Type { name, fields, records });
        }
        Ok(Self { data, blob, blob_len, cells, types })
    }

    fn string(&self, offset: u32) -> Result<String> {
        cstr_at(&self.data, self.blob + offset as usize)
    }

    /// The record `name` of the type `type_name`, e.g. `("Model", "Cha_Player_Talion_PD")`.
    pub fn record(&self, type_name: &str, name: &str) -> Option<Entry<'_>> {
        let ty = self.types.iter().find(|t| self.string(t.name).is_ok_and(|n| n == type_name))?;
        let item = ty.records.iter().find(|r| {
            r.name.is_some_and(|n| self.string(n).is_ok_and(|s| s.eq_ignore_ascii_case(name)))
        })?;
        Some(Entry { db: self, item })
    }

    /// Follow a reference value (`type index << 20 | item index`).
    pub fn follow(&self, reference: u32) -> Option<Entry<'_>> {
        let ty = self.types.get((reference >> 20) as usize)?;
        let index = (reference & 0xf_ffff) as usize;
        let item = match index.checked_sub(ty.fields.len()) {
            None => &ty.fields[index],
            Some(record) => ty.records.get(record)?,
        };
        Some(Entry { db: self, item })
    }
}

fn read_item(data: &[u8], at: &mut usize, named: bool) -> Result<Item> {
    let name = if named {
        let n = u32_at(data, *at)?;
        *at += 4;
        Some(n)
    } else {
        None
    };
    let raw = data.get(*at..*at + 8).ok_or_else(|| anyhow!("item header truncated"))?;
    let value_count = u16::from_le_bytes([raw[0], raw[1]]) as usize;
    let cell_count = raw[2];
    let first_cell = u32::from_le_bytes(raw[3..7].try_into().unwrap());
    *at += 8;
    let mut values = Vec::with_capacity(value_count);
    for i in 0..value_count {
        values.push(u32_at(data, *at + 4 * i)?);
    }
    *at += 4 * value_count;
    Ok(Item { name, first_cell, cell_count, values })
}

impl GameDb {
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// `(name, inline struct count, record count)` of type `index`.
    pub fn type_info(&self, index: usize) -> Option<(String, usize, usize)> {
        let t = self.types.get(index)?;
        Some((self.string(t.name).unwrap_or_default(), t.fields.len(), t.records.len()))
    }

    /// Item `item` of type `index` (inline structs first, then records), as [`GameDb::follow`] counts them.
    pub fn item(&self, index: usize, item: usize) -> Option<Entry<'_>> {
        self.follow((index as u32) << 20 | item as u32)
    }

    /// The `f32` stored at byte `offset` of the string pool (range fields keep their numbers there).
    pub fn pool_f32(&self, offset: u32) -> Option<f32> {
        let at = self.blob + offset as usize;
        self.data.get(at..at + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap()))
    }

    /// Every string of the string pool.
    pub fn strings(&self) -> Vec<String> {
        let end = self.blob_len;
        let mut out = Vec::new();
        let mut at = 0;
        while at < end {
            let s = cstr_at(&self.data, self.blob + at).unwrap_or_default();
            at += s.len() + 1;
            out.push(s);
        }
        out
    }
}

impl<'a> Entry<'a> {
    /// The raw 32-bit values of the entry, before the cells interpret them.
    pub fn raw_values(&self) -> &'a [u32] {
        &self.item.values
    }

    pub fn name(&self) -> Option<String> {
        self.item.name.and_then(|n| self.db.string(n).ok())
    }

    pub fn cells(&self) -> &'a [Cell] {
        let first = self.item.first_cell as usize;
        &self.db.cells[first..first + self.item.cell_count as usize]
    }

    /// The field whose name hash is `hash`.
    pub fn field(&self, hash: u32) -> Option<Value> {
        let cell = self.cells().iter().find(|c| c.hash == hash)?;
        Some(self.value(cell))
    }

    /// The raw 32-bit values of the field `hash`, whatever its kind (kind 7 fields point into the float pool).
    pub fn raw_field(&self, hash: u32) -> Option<&'a [u32]> {
        let cell = self.cells().iter().find(|c| c.hash == hash)?;
        let at = cell.first as usize;
        self.item.values.get(at..at + cell.count as usize)
    }

    pub fn value(&self, cell: &Cell) -> Value {
        let at = cell.first as usize;
        let raw = self.item.values.get(at..at + cell.count as usize).unwrap_or(&[]);
        match cell.kind {
            4..=9 => Value::Strings(raw.iter().map(|&o| self.db.string(o).unwrap_or_default()).collect()),
            11 => Value::Refs(raw.to_vec()),
            kind => Value::Raw { kind, values: raw.to_vec() },
        }
    }

    /// Every record reference held by this entry's fields.
    pub fn references(&self) -> Vec<u32> {
        self.cells()
            .iter()
            .filter(|c| c.kind == 11)
            .flat_map(|c| match self.value(c) {
                Value::Refs(r) => r,
                _ => Vec::new(),
            })
            .collect()
    }
}

/// The material paths of every material set of a `Model` record, in mesh slot order.
///
/// Set 0 is the normal look; later sets are variants (the dissolve `_fade` materials).
pub fn model_material_sets(db: &GameDb, model: &str) -> Result<Vec<Vec<String>>> {
    let record = db.record("Model", model).ok_or_else(|| anyhow!("no Model record {model}"))?;
    let mut sets = Vec::new();
    for reference in record.references() {
        let Some(set) = db.follow(reference) else { continue };
        if let Some(Value::Strings(paths)) = set.field(MATERIAL_PATHS) {
            sets.push(paths);
        }
    }
    Ok(sets)
}
