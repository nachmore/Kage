//! Adobe's CID to Unicode tables for the four CJK orderings
//! (EXTENSION_FILE_TEXT §6.2 step 7).
//!
//! Source: the `Adobe-Japan1-UCS2`, `Adobe-Korea1-UCS2`, `Adobe-GB1-UCS2`
//! and `Adobe-CNS1-UCS2` CMaps of Adobe's `mapping-resources-pdf`
//! (`pdf2unicode/`), BSD-3-Clause; the licence is `cmaps/LICENSE.txt`.
//! The `regenerate_adobe_tables` test turns them into the compact files
//! under `cmaps/` (see `cmaps/README.md`); this module only reads those.
//!
//! Two changes from the source: variation selectors are dropped (a CID
//! for a glyph variant maps to its base character, so a search finds it),
//! and `U+FFFD` entries are left out (unmapped, so no replacement
//! character ends up in the text).
//!
//! File format, all numbers LEB128 varints:
//!
//! ```text
//! "KCID" 1
//! n                      CIDs in the table (0..n)
//! n × zigzag(delta)      each entry minus the previous one (the first minus 0)
//! m                      strings in the pool
//! m × (len, len × u)     each string as Unicode scalar values
//! ```
//!
//! An entry is 0 (no mapping), a scalar value (one character), or
//! `POOL_BASE + i` for pool string `i` (more than one character).

const MAGIC: &[u8; 5] = b"KCID\x01";
/// Entries at or above this point into the string pool.
const POOL_BASE: u32 = 0x11_0000;
/// The highest CID an ordering uses is about 30,000.
const MAX_CIDS: usize = 1 << 16;
const MAX_POOL: usize = 1 << 16;
const MAX_POOL_STRING: usize = 32;

/// One Adobe CJK character collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ordering {
    Japan1,
    Korea1,
    Gb1,
    Cns1,
}

impl Ordering {
    pub const ALL: [Ordering; 4] = [
        Ordering::Japan1,
        Ordering::Korea1,
        Ordering::Gb1,
        Ordering::Cns1,
    ];

    /// The ordering a `/CIDSystemInfo` names, if Adobe's.
    pub fn from_names(registry: &[u8], ordering: &[u8]) -> Option<Self> {
        if registry != b"Adobe" {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|o| o.name().as_bytes() == ordering)
    }

    /// `Japan1`, `Korea1`, `GB1` or `CNS1`.
    pub const fn name(self) -> &'static str {
        match self {
            Ordering::Japan1 => "Japan1",
            Ordering::Korea1 => "Korea1",
            Ordering::Gb1 => "GB1",
            Ordering::Cns1 => "CNS1",
        }
    }

    /// The compact table built into the worker.
    pub fn data(self) -> &'static [u8] {
        match self {
            Ordering::Japan1 => include_bytes!("cmaps/adobe-japan1.bin"),
            Ordering::Korea1 => include_bytes!("cmaps/adobe-korea1.bin"),
            Ordering::Gb1 => include_bytes!("cmaps/adobe-gb1.bin"),
            Ordering::Cns1 => include_bytes!("cmaps/adobe-cns1.bin"),
        }
    }

    /// The decoded table. `None` only if the built-in file is broken,
    /// which a unit test rules out.
    pub fn table(self) -> Option<CidTable> {
        CidTable::decode(self.data())
    }
}

/// CID to Unicode for one ordering.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CidTable {
    entries: Vec<u32>,
    pool: Vec<String>,
}

/// What a CID maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapping<'a> {
    Char(char),
    Text(&'a str),
}

impl CidTable {
    fn mapping(&self, entry: u32) -> Option<Mapping<'_>> {
        match entry {
            0 => None,
            e if e < POOL_BASE => char::from_u32(e).map(Mapping::Char),
            e => self
                .pool
                .get((e - POOL_BASE) as usize)
                .map(|t| Mapping::Text(t)),
        }
    }

    /// What a CID maps to, if anything.
    pub fn lookup(&self, cid: u32) -> Option<Mapping<'_>> {
        self.mapping(*self.entries.get(cid as usize)?)
    }

    /// Every mapped CID with its text, in CID order.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = (u32, Mapping<'_>)> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(move |(cid, &entry)| Some((cid as u32, self.mapping(entry)?)))
    }

    /// Reads the compact form. `None` if it is broken in any way.
    pub fn decode(bytes: &[u8]) -> Option<CidTable> {
        let mut input = bytes.strip_prefix(MAGIC.as_slice())?;
        let n = usize::try_from(take(&mut input)?).ok()?;
        if n > MAX_CIDS {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        let mut previous = 0i64;
        for _ in 0..n {
            let z = take(&mut input)?;
            let delta = (z >> 1) as i64 ^ -((z & 1) as i64);
            previous = previous.checked_add(delta)?;
            entries.push(u32::try_from(previous).ok()?);
        }
        let m = usize::try_from(take(&mut input)?).ok()?;
        if m > MAX_POOL {
            return None;
        }
        let mut pool = Vec::with_capacity(m);
        for _ in 0..m {
            let len = usize::try_from(take(&mut input)?).ok()?;
            if len > MAX_POOL_STRING {
                return None;
            }
            let mut text = String::new();
            for _ in 0..len {
                text.push(char::from_u32(u32::try_from(take(&mut input)?).ok()?)?);
            }
            pool.push(text);
        }
        let valid = input.is_empty()
            && entries.iter().all(|&e| match e.checked_sub(POOL_BASE) {
                None => char::from_u32(e).is_some(),
                Some(i) => (i as usize) < m,
            });
        valid.then_some(CidTable { entries, pool })
    }
}

/// One LEB128 varint.
fn take(input: &mut &[u8]) -> Option<u64> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        n |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

#[cfg(test)]
#[path = "cid_tables_tests.rs"]
mod tests;
