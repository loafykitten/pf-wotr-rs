//! UnityFS asset bundles (`Bundles/*`): header, block table, directory and block
//! decompression. Format as written by Unity 2020.3.48f1 (archive format 8).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use super::reader::{Endian, Reader};

/// Archive flags (Unity 2020.3.34+ meaning).
const COMPRESSION_MASK: u32 = 0x3f;
const BLOCKS_INFO_AT_END: u32 = 0x80;
const BLOCK_INFO_NEEDS_PADDING_AT_START: u32 = 0x200;

/// One file inside a bundle (a serialized file such as `CAB-…`, or a `.resS` stream).
#[derive(Debug, Clone)]
pub struct Node {
    pub offset: u64,
    pub size: u64,
    pub flags: u32,
    pub path: String,
}

#[derive(Debug, Clone)]
struct Block {
    uncompressed: u32,
    compressed: u32,
    flags: u16,
}

/// Parsed bundle header plus block table and directory.
#[derive(Debug, Clone)]
pub struct BundleHeader {
    pub format: u32,
    pub unity_version: String,
    pub file_size: i64,
    pub flags: u32,
    pub nodes: Vec<Node>,
    blocks: Vec<Block>,
    /// Offset of the first data block in the file.
    data_start: usize,
}

impl BundleHeader {
    /// Parses the header from the start of the file. `bytes` may be a prefix of the file
    /// as long as it covers the header and block table (unless the table is at the end).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_inner(bytes, true)
    }

    /// `complete` says whether `bytes` is the whole file; a block table stored at the end
    /// can only be read from a complete file.
    fn parse_inner(bytes: &[u8], complete: bool) -> Result<Self> {
        let mut r = Reader::new(bytes, Endian::Big);
        let sig = r.cstr()?;
        ensure!(sig == "UnityFS", "not a UnityFS bundle (signature {sig:?})");
        let format = r.u32()?;
        let _player = r.cstr()?;
        let unity_version = r.cstr()?;
        let file_size = r.i64()?;
        let compressed = r.u32()? as usize;
        let uncompressed = r.u32()? as usize;
        let flags = r.u32()?;
        if format >= 7 {
            r.align(16)?;
        }
        let after_header = r.pos();
        let info_raw = if flags & BLOCKS_INFO_AT_END != 0 {
            ensure!(complete, "block info at end needs the whole file");
            let len = bytes.len();
            ensure!(compressed <= len, "block info at end needs the whole file");
            &bytes[len - compressed..]
        } else {
            r.take(compressed)?
        };
        let info =
            decompress(info_raw, uncompressed, flags & COMPRESSION_MASK).context("block info")?;
        let mut bi = Reader::new(&info, Endian::Big);
        bi.take(16)?; // uncompressed data hash
        let nblocks = bi.len_i32(10)?;
        let mut blocks = Vec::with_capacity(nblocks);
        for _ in 0..nblocks {
            blocks.push(Block { uncompressed: bi.u32()?, compressed: bi.u32()?, flags: bi.u16()? });
        }
        let nnodes = bi.len_i32(21)?;
        let mut nodes = Vec::with_capacity(nnodes);
        for _ in 0..nnodes {
            nodes.push(Node {
                offset: bi.u64()?,
                size: bi.u64()?,
                flags: bi.u32()?,
                path: bi.cstr()?,
            });
        }
        let mut data_start =
            if flags & BLOCKS_INFO_AT_END != 0 { after_header } else { after_header + compressed };
        if flags & BLOCK_INFO_NEEDS_PADDING_AT_START != 0 {
            data_start = data_start.next_multiple_of(16);
        }
        Ok(Self { format, unity_version, file_size, flags, nodes, blocks, data_start })
    }

    /// Reads only as much of the file as the header needs (directory listing).
    pub fn read_from_file(path: &Path) -> Result<Self> {
        let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut buf = vec![0u8; 64 * 1024];
        let n = read_up_to(&mut f, &mut buf)?;
        buf.truncate(n);
        let complete = n < 64 * 1024;
        match Self::parse_inner(&buf, complete) {
            Ok(h) => Ok(h),
            Err(_) => {
                // Large directory or block table at the end: fall back to the whole file.
                let all = std::fs::read(path)?;
                Self::parse(&all)
            }
        }
    }
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

/// A fully decompressed bundle.
#[derive(Debug)]
pub struct Bundle {
    pub header: BundleHeader,
    data: Vec<u8>,
}

impl Bundle {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::from_bytes(&bytes).with_context(|| format!("bundle {}", path.display()))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let header = BundleHeader::parse(bytes)?;
        let total: usize = header.blocks.iter().map(|b| b.uncompressed as usize).sum();
        let mut data = Vec::with_capacity(total);
        let mut pos = header.data_start;
        for (i, b) in header.blocks.iter().enumerate() {
            let end = pos + b.compressed as usize;
            ensure!(end <= bytes.len(), "block {i} overruns file");
            let out = decompress(
                &bytes[pos..end],
                b.uncompressed as usize,
                u32::from(b.flags) & COMPRESSION_MASK,
            )
            .with_context(|| format!("block {i}"))?;
            data.extend_from_slice(&out);
            pos = end;
        }
        Ok(Self { header, data })
    }

    /// Bytes of a node (a file inside the bundle).
    pub fn node_data(&self, node: &Node) -> Result<&[u8]> {
        let start = usize::try_from(node.offset)?;
        let end = start + usize::try_from(node.size)?;
        ensure!(end <= self.data.len(), "node {} overruns bundle data", node.path);
        Ok(&self.data[start..end])
    }

    pub fn node(&self, path: &str) -> Option<&Node> {
        self.header.nodes.iter().find(|n| n.path == path)
    }
}

/// Decompresses one block. `kind` is Unity's compression type: 0 none, 1 LZMA, 2 LZ4,
/// 3 LZ4HC.
pub fn decompress(src: &[u8], uncompressed: usize, kind: u32) -> Result<Vec<u8>> {
    match kind {
        0 => {
            ensure!(src.len() == uncompressed, "stored block size mismatch");
            Ok(src.to_vec())
        }
        1 => {
            // Unity's LZMA blocks: 5-byte properties, no size field, raw stream.
            let mut out = Vec::with_capacity(uncompressed);
            let opts = lzma_rs::decompress::Options {
                unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(
                    uncompressed as u64,
                )),
                ..Default::default()
            };
            lzma_rs::lzma_decompress_with_options(&mut &src[..], &mut out, &opts)
                .map_err(|e| anyhow::anyhow!("lzma: {e:?}"))?;
            Ok(out)
        }
        2 | 3 => {
            let out = lz4_flex::block::decompress(src, uncompressed)?;
            ensure!(out.len() == uncompressed, "lz4 size mismatch");
            Ok(out)
        }
        k => bail!("unsupported compression type {k}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal uncompressed UnityFS archive in memory (format 8).
    fn fake_bundle(payload: &[u8]) -> Vec<u8> {
        let mut info = Vec::new();
        info.extend_from_slice(&[0u8; 16]);
        info.extend_from_slice(&1i32.to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        info.extend_from_slice(&0u16.to_be_bytes());
        info.extend_from_slice(&1i32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        info.extend_from_slice(&4u32.to_be_bytes());
        info.extend_from_slice(b"CAB-test\0");
        let mut b = Vec::new();
        b.extend_from_slice(b"UnityFS\0");
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\0");
        b.extend_from_slice(b"2020.3.48f1\0");
        b.extend_from_slice(&0i64.to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&(0x40u32 | 0x200).to_be_bytes());
        while b.len() % 16 != 0 {
            b.push(0);
        }
        b.extend_from_slice(&info);
        while b.len() % 16 != 0 {
            b.push(0);
        }
        b.extend_from_slice(payload);
        b
    }

    #[test]
    fn parses_a_minimal_archive() {
        let bytes = fake_bundle(b"hello world");
        let b = Bundle::from_bytes(&bytes).unwrap();
        assert_eq!(b.header.format, 8);
        assert_eq!(b.header.unity_version, "2020.3.48f1");
        let n = b.node("CAB-test").unwrap();
        assert_eq!(b.node_data(n).unwrap(), b"hello world");
    }

    #[test]
    fn lz4_round_trip() {
        let src = b"abcabcabcabcabcabcabcabc".repeat(10);
        let c = lz4_flex::block::compress(&src);
        assert_eq!(decompress(&c, src.len(), 3).unwrap(), src);
        assert!(decompress(&c, src.len(), 9).is_err());
    }
}
