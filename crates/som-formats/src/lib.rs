//! Readers for Middle-earth: Shadow of Mordor (PC) data formats.
//!
//! Everything here reads the user's own install in place; nothing is extracted to disk
//! unless a tool asks for it.

pub mod animator;
pub mod anix;
pub mod bndl;
pub mod bvrgraph;
pub mod ctxsearch;
pub mod combatdb;
pub mod banks;
pub mod cntx;
pub mod combatnodes;
pub mod gamedb;
pub mod hash;
pub mod ltar;
pub mod math;
pub mod skel;
pub mod sync;
pub mod vfs;

/// Canonical form of an asset path: lowercase, forward slashes.
pub fn normalize_path(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

pub(crate) fn u32_at(buf: &[u8], off: usize) -> anyhow::Result<u32> {
    let bytes = buf
        .get(off..off + 4)
        .ok_or_else(|| anyhow::anyhow!("read past end at {off:#x}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

pub(crate) fn u64_at(buf: &[u8], off: usize) -> anyhow::Result<u64> {
    let bytes = buf
        .get(off..off + 8)
        .ok_or_else(|| anyhow::anyhow!("read past end at {off:#x}"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

pub(crate) fn cstr_at(buf: &[u8], off: usize) -> anyhow::Result<String> {
    let tail = buf
        .get(off..)
        .ok_or_else(|| anyhow::anyhow!("string offset {off:#x} out of range"))?;
    let end = tail
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| anyhow::anyhow!("unterminated string at {off:#x}"))?;
    Ok(tail[..end].iter().map(|&b| b as char).collect())
}
