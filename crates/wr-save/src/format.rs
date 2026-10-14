//! Our save format, version 1: a snapshot of every decoded document plus the bytes of
//! every other entry, one independently compressed chunk per entry.
//!
//! ```text
//! header   magic "WRSAVE\0\x01" | format version u32 | schema fingerprint u64 | entry count u32
//! index    per entry: name length u16 | name | kind u8 | raw length u64 | stored length u64
//! chunks   per entry: zstd(postcard(Body))
//! ```
//!
//! All integers little-endian. Chunks are independent, so encoding and decoding run in
//! parallel and a loader can read one area's documents without the rest (the basis for
//! incremental saves). A file written with another schema is refused: the typed values
//! index the schema's types, members and enums.
//!
//! This first version stores the decoded JSON documents (typed values plus the opaque
//! remainder). Once `wr-sim` exists the snapshot will hold the simulation's own state, with
//! the same container and the preserved remainder carried alongside.

use anyhow::{Context, Result, bail, ensure};
use rayon::prelude::*;

use crate::schema;
use crate::zks::Body;

pub const MAGIC: &[u8; 8] = b"WRSAVE\0\x01";
pub const VERSION: u32 = 1;
/// zstd level: 3 is zstd's default; higher levels cost encode time for little size.
pub const LEVEL: i32 = 3;

/// One entry of a snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub body: Body,
}

fn put_u16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.i.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else { bail!("truncated snapshot") };
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
}

/// Encodes a snapshot.
///
/// # Errors
///
/// Serialization or compression failure, or a name longer than 65535 bytes.
pub fn encode(items: &[Item]) -> Result<Vec<u8>> {
    let chunks: Vec<Result<(u8, u64, Vec<u8>)>> = items
        .par_iter()
        .map(|it| {
            let kind = match &it.body {
                Body::Doc(_) => 0u8,
                Body::Bytes(_) => 1u8,
            };
            let raw = postcard::to_allocvec(&it.body)?;
            let packed = zstd::bulk::compress(&raw, LEVEL)?;
            Ok((kind, raw.len() as u64, packed))
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, VERSION);
    put_u64(&mut out, schema::fingerprint());
    put_u32(&mut out, u32::try_from(items.len())?);
    let mut data = Vec::new();
    for (it, c) in items.iter().zip(chunks) {
        let (kind, raw, packed) = c?;
        put_u16(&mut out, u16::try_from(it.name.len())?);
        out.extend_from_slice(it.name.as_bytes());
        out.push(kind);
        put_u64(&mut out, raw);
        put_u64(&mut out, packed.len() as u64);
        data.push(packed);
    }
    for d in data {
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// Index entry of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: String,
    pub kind: u8,
    pub raw_len: u64,
    pub offset: usize,
    pub len: usize,
}

/// Reads and validates the header and index.
///
/// # Errors
///
/// Wrong magic, version or schema, or an index that doesn't fit the file.
pub fn index(bytes: &[u8]) -> Result<Vec<IndexEntry>> {
    let mut c = Cursor { b: bytes, i: 0 };
    ensure!(c.take(8)? == MAGIC, "not a wr-save snapshot");
    let v = c.u32()?;
    ensure!(v == VERSION, "snapshot format version {v}, expected {VERSION}");
    ensure!(c.u64()? == schema::fingerprint(), "snapshot written with another save schema");
    let n = c.u32()? as usize;
    let mut idx = Vec::with_capacity(n.min(1 << 16));
    let mut pending = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let len = usize::from(c.u16()?);
        let name = std::str::from_utf8(c.take(len)?)?.to_owned();
        let kind = c.u8()?;
        ensure!(kind <= 1, "bad entry kind {kind}");
        let raw_len = c.u64()?;
        let stored = usize::try_from(c.u64()?)?;
        pending.push((name, kind, raw_len, stored));
    }
    let mut off = c.i;
    for (name, kind, raw_len, len) in pending {
        let end = off.checked_add(len).filter(|&e| e <= bytes.len());
        ensure!(end.is_some(), "chunk for {name} runs past the end");
        idx.push(IndexEntry { name, kind, raw_len, offset: off, len });
        off += len;
    }
    ensure!(off == bytes.len(), "trailing bytes after the last chunk");
    Ok(idx)
}

/// Decodes one entry's chunk.
///
/// # Errors
///
/// Corrupt compression or serialization.
pub fn read_entry(bytes: &[u8], e: &IndexEntry) -> Result<Item> {
    let chunk = &bytes[e.offset..e.offset + e.len];
    // Guard the allocation against a corrupt length: chunks are at most a few hundred MB.
    ensure!(e.raw_len < 1 << 32, "implausible chunk size for {}", e.name);
    let raw = zstd::bulk::decompress(chunk, usize::try_from(e.raw_len)?)?;
    ensure!(raw.len() as u64 == e.raw_len, "chunk length mismatch for {}", e.name);
    let body: Body = postcard::from_bytes(&raw)?;
    if let Body::Doc(d) = &body {
        validate(d).with_context(|| format!("invalid value in {}", e.name))?;
    }
    let kind_ok = matches!((&body, e.kind), (Body::Doc(_), 0) | (Body::Bytes(_), 1));
    ensure!(kind_ok, "entry kind mismatch for {}", e.name);
    Ok(Item { name: e.name.clone(), body })
}

/// Checks that every schema index in a decoded document is in range (a matching schema
/// fingerprint doesn't prove a corrupt or forged body is consistent).
///
/// # Errors
///
/// The first out-of-range index.
pub fn validate(d: &crate::value::Doc) -> Result<()> {
    use crate::value::{ENUM_NUMBER, Key, NO_TAG, Value};
    let types = schema::types().len();
    let mut err: Option<String> = None;
    d.root.walk(&mut |v| {
        if err.is_some() {
            return;
        }
        let bad_enum = |e: u16, name: u16| {
            let ok = schema::enum_count() > usize::from(e)
                && (name == ENUM_NUMBER
                    || usize::from(name) < schema::enum_def(u32::from(e)).members.len());
            (!ok).then(|| format!("enum {e}/{name}"))
        };
        err = match v {
            Value::Obj(o) if o.ty as usize >= types => Some(format!("type {}", o.ty)),
            Value::Obj(o) if o.tag != NO_TAG && o.tag as usize >= d.tags.len() => {
                Some(format!("tag {}", o.tag))
            }
            Value::Obj(o) => {
                let n = schema::ty(o.ty).members.len();
                o.props.iter().find_map(|p| match p.key {
                    Key::Member(i) | Key::MemberCase(i, _) if usize::from(i) >= n => {
                        Some(format!("member {i} of type {}", o.ty))
                    }
                    _ => None,
                })
            }
            Value::Enum { e, name, .. } => bad_enum(*e, *name),
            Value::EnumText(t) => bad_enum(t.e, ENUM_NUMBER),
            _ => None,
        };
    });
    match err {
        Some(e) => bail!("out-of-range index: {e}"),
        None => Ok(()),
    }
}

/// Decodes a whole snapshot (entries in parallel).
///
/// # Errors
///
/// See [`index`] and [`read_entry`].
pub fn decode(bytes: &[u8]) -> Result<Vec<Item>> {
    let idx = index(bytes)?;
    idx.par_iter().map(|e| read_entry(bytes, e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Doc, Value};

    #[test]
    fn round_trip_and_corruption() {
        let items = vec![
            Item {
                name: "a.json".into(),
                body: Body::Doc(Doc { bom: true, root: Value::Int(5), tags: vec![] }),
            },
            Item { name: "b.fog".into(), body: Body::Bytes(vec![1, 2, 3]) },
        ];
        let bytes = encode(&items).unwrap();
        assert_eq!(decode(&bytes).unwrap(), items);
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad.truncate(bytes.len() - 1);
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[12] ^= 1; // schema fingerprint
        assert!(decode(&bad).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode(&bad).is_err());
        // A well-formed body with an out-of-range type index is refused, not trusted.
        let forged = vec![Item {
            name: "c.json".into(),
            body: Body::Doc(Doc {
                bom: false,
                root: Value::Obj(Box::new(crate::value::Obj {
                    ty: u32::MAX - 1,
                    tag: crate::value::NO_TAG,
                    id: None,
                    type_first: false,
                    props: vec![],
                })),
                tags: vec![],
            }),
        }];
        assert!(decode(&encode(&forged).unwrap()).is_err());
    }
}
