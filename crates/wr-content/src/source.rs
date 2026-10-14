//! Reading `blueprints.zip` without unpacking it: the archive is read into memory once
//! and entries are inflated in parallel from shared clones of the central directory.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rayon::prelude::*;

/// An in-memory reader that can be cloned cheaply (shares the bytes).
#[derive(Debug, Clone)]
struct SharedCursor {
    data: Arc<[u8]>,
    pos: u64,
}

impl Read for SharedCursor {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = usize::try_from(self.pos).unwrap_or(usize::MAX).min(self.data.len());
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SharedCursor {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let len = self.data.len() as u64;
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match new {
            Some(p) => {
                self.pos = p;
                Ok(p)
            }
            None => Err(std::io::Error::other("seek before start")),
        }
    }
}

/// `blueprints.zip`, opened read-only.
#[derive(Debug)]
pub struct BlueprintZip {
    archive: zip::ZipArchive<SharedCursor>,
    /// Indices and names of the `.jbp` entries, in archive order.
    entries: Vec<(usize, String)>,
    /// Size of the archive file and a hash of its central directory, identifying the input.
    pub fingerprint: (u64, u64),
}

impl BlueprintZip {
    /// Reads the archive into memory and lists its `.jbp` entries.
    ///
    /// # Errors
    ///
    /// I/O or zip format errors.
    pub fn open(path: &Path) -> Result<Self> {
        let data: Arc<[u8]> =
            std::fs::read(path).with_context(|| format!("read {}", path.display()))?.into();
        let len = data.len() as u64;
        let archive = zip::ZipArchive::new(SharedCursor { data, pos: 0 })?;
        let mut entries = Vec::new();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for i in 0..archive.len() {
            let name = archive
                .name_for_index(i)
                .and_then(Result::ok)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default();
            for b in name.bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            if name.ends_with(".jbp") {
                entries.push((i, name));
            }
        }
        // Mix in the uncompressed sizes and CRCs via a sample of entries' metadata.
        let mut a = archive.clone();
        for (i, _) in entries.iter().step_by(97) {
            if let Ok(f) = a.by_index_raw(*i) {
                h = (h ^ u64::from(f.crc32())).wrapping_mul(0x0100_0000_01b3);
                h = (h ^ f.size()).wrapping_mul(0x0100_0000_01b3);
            }
        }
        Ok(Self { archive, entries, fingerprint: (len, h) })
    }

    /// Paths of the `.jbp` entries, in archive order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(_, n)| n.as_str())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Reads one entry by its position among the `.jbp` entries.
    ///
    /// # Errors
    ///
    /// Zip or inflate errors.
    pub fn read(&self, n: usize) -> Result<Vec<u8>> {
        let (i, _) = &self.entries[n];
        let mut a = self.archive.clone();
        let mut f = a.by_index(*i)?;
        let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Runs `f(position, path, bytes)` over every `.jbp` in parallel and returns the
    /// results in archive order. Each worker inflates from its own clone of the archive.
    pub fn par_map<T: Send>(
        &self,
        f: impl Fn(usize, &str, Result<Vec<u8>>) -> T + Sync + Send,
    ) -> Vec<T> {
        let chunk = 512;
        let idx: Vec<usize> = (0..self.entries.len()).step_by(chunk).collect();
        idx.par_iter()
            .flat_map_iter(|&start| {
                let mut a = self.archive.clone();
                let end = (start + chunk).min(self.entries.len());
                let mut out = Vec::with_capacity(end - start);
                let mut buf = Vec::new();
                for n in start..end {
                    let (i, name) = &self.entries[n];
                    buf.clear();
                    let r = a
                        .by_index(*i)
                        .map_err(anyhow::Error::from)
                        .and_then(|mut e| e.read_to_end(&mut buf).map_err(anyhow::Error::from))
                        .map(|_| std::mem::take(&mut buf));
                    out.push(f(n, name, r));
                }
                out
            })
            .collect()
    }

    /// Folds over every `.jbp` in parallel: each chunk of entries gets a fresh
    /// accumulator from `init`, and the chunks' accumulators are returned in order.
    pub fn par_fold<A: Send>(
        &self,
        init: impl Fn() -> A + Sync + Send,
        f: impl Fn(&mut A, usize, &str, Result<Vec<u8>>) + Sync + Send,
    ) -> Vec<A> {
        let chunk = 512;
        let idx: Vec<usize> = (0..self.entries.len()).step_by(chunk).collect();
        idx.par_iter()
            .map(|&start| {
                let mut a = self.archive.clone();
                let end = (start + chunk).min(self.entries.len());
                let mut acc = init();
                for n in start..end {
                    let (i, name) = &self.entries[n];
                    let r = a.by_index(*i).map_err(anyhow::Error::from).and_then(|mut e| {
                        let mut buf = Vec::with_capacity(usize::try_from(e.size()).unwrap_or(0));
                        e.read_to_end(&mut buf).map_err(anyhow::Error::from).map(|_| buf)
                    });
                    f(&mut acc, n, name, r);
                }
                acc
            })
            .collect()
    }
}
