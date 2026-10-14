//! A bounds-checked cursor over a byte slice with switchable endianness.

use anyhow::{Result, bail};

/// Byte order of the data being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// Cursor over borrowed bytes. Every read is bounds-checked and fails with an error
/// rather than panicking, because the input is untrusted game data.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    pub endian: Endian,
}

macro_rules! read_num {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self) -> Result<$t> {
            let b = self.take(size_of::<$t>())?;
            let arr = b.try_into().expect("take returned the requested length");
            Ok(match self.endian {
                Endian::Little => <$t>::from_le_bytes(arr),
                Endian::Big => <$t>::from_be_bytes(arr),
            })
        }
    };
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], endian: Endian) -> Self {
        Self { data, pos: 0, endian }
    }

    #[must_use]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            bail!("seek to {pos} past end ({})", self.data.len());
        }
        self.pos = pos;
        Ok(())
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            bail!("read of {n} bytes at {} overruns {} bytes", self.pos, self.data.len());
        };
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Advances to the next multiple of `n` (relative to the start of the slice).
    pub fn align(&mut self, n: usize) -> Result<()> {
        let rem = self.pos % n;
        if rem != 0 {
            self.take(n - rem)?;
        }
        Ok(())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()?.cast_signed())
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    read_num!(u16, u16);
    read_num!(i16, i16);
    read_num!(u32, u32);
    read_num!(i32, i32);
    read_num!(u64, u64);
    read_num!(i64, i64);
    read_num!(f32, f32);
    read_num!(f64, f64);

    /// A NUL-terminated UTF-8 string (lossy).
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let Some(n) = rest.iter().position(|&b| b == 0) else {
            bail!("unterminated string at {}", self.pos);
        };
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.pos += n + 1;
        Ok(s)
    }

    /// A non-negative `i32` length, checked against the bytes left so that corrupt
    /// counts fail fast instead of allocating.
    pub fn len_i32(&mut self, min_elem_size: usize) -> Result<usize> {
        let n = self.i32()?;
        let Ok(n) = usize::try_from(n) else { bail!("negative length {n} at {}", self.pos) };
        if n.saturating_mul(min_elem_size) > self.remaining() {
            bail!("length {n} at {} exceeds remaining {} bytes", self.pos, self.remaining());
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_endians_and_aligns() {
        let d = [1u8, 0, 0, 0, 0, 0, 0, 2, 0xAA, b'h', b'i', 0];
        let mut r = Reader::new(&d, Endian::Little);
        assert_eq!(r.u32().unwrap(), 1);
        r.endian = Endian::Big;
        assert_eq!(r.u32().unwrap(), 2);
        assert_eq!(r.u8().unwrap(), 0xAA);
        assert_eq!(r.cstr().unwrap(), "hi");
        assert!(r.u8().is_err());
        let mut r = Reader::new(&d, Endian::Little);
        r.u8().unwrap();
        r.align(4).unwrap();
        assert_eq!(r.pos(), 4);
    }

    #[test]
    fn rejects_absurd_lengths() {
        let d = 1000i32.to_le_bytes();
        let mut r = Reader::new(&d, Endian::Little);
        assert!(r.len_i32(1).is_err());
    }
}
