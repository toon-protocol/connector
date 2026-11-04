use std::marker::PhantomData;

use binprot::Nat0;

const MINA_STRING_MAX_LENGTH: usize = 100_000_000;
const CHUNK_SIZE: usize = 5_000;

pub type ByteString = BoundedByteString<MINA_STRING_MAX_LENGTH>;

// <https://github.com/MinaProtocol/mina/blob/c0c9d702b8cba34a603a28001c293ca462b1dfec/src/lib/mina_base/zkapp_account.ml#L140>
// <https://github.com/MinaProtocol/mina/blob/c0c9d702b8cba34a603a28001c293ca462b1dfec/src/lib/mina_base/account.ml#L92>

/// String of bytes.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoundedByteString<const MAX_LENGTH: usize>(pub Vec<u8>, PhantomData<[u8; MAX_LENGTH]>);

impl<const MAX_LENGTH: usize> std::fmt::Debug for BoundedByteString<MAX_LENGTH> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(inner, _) = self;
        // Avoid vertical alignment
        f.write_fmt(format_args!("BoundedByteString<{MAX_LENGTH}>({:?})", inner))
    }
}

impl<const MAX_LENGTH: usize> std::ops::Deref for BoundedByteString<MAX_LENGTH> {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const MAX_LENGTH: usize> AsRef<[u8]> for BoundedByteString<MAX_LENGTH> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const MAX_LENGTH: usize> From<Vec<u8>> for BoundedByteString<MAX_LENGTH> {
    fn from(source: Vec<u8>) -> Self {
        Self(source, PhantomData)
    }
}

impl<const MAX_LENGTH: usize> From<&[u8]> for BoundedByteString<MAX_LENGTH> {
    fn from(source: &[u8]) -> Self {
        Self(source.to_vec(), PhantomData)
    }
}

impl<const MAX_LENGTH: usize> From<&str> for BoundedByteString<MAX_LENGTH> {
    fn from(source: &str) -> Self {
        Self(source.as_bytes().to_vec(), PhantomData)
    }
}

impl<const MAX_LENGTH: usize> TryFrom<BoundedByteString<MAX_LENGTH>> for String {
    type Error = std::string::FromUtf8Error;

    fn try_from(value: BoundedByteString<MAX_LENGTH>) -> Result<Self, Self::Error> {
        String::from_utf8(value.0)
    }
}

impl<const MAX_LENGTH: usize> TryFrom<&BoundedByteString<MAX_LENGTH>> for String {
    type Error = std::string::FromUtf8Error;

    fn try_from(value: &BoundedByteString<MAX_LENGTH>) -> Result<Self, Self::Error> {
        String::from_utf8(value.0.clone())
    }
}

impl<const MAX_LENGTH: usize> binprot::BinProtRead for BoundedByteString<MAX_LENGTH> {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        let len = Nat0::binprot_read(r)?.0 as usize;
        if len > MAX_LENGTH {
            return Err(MinaStringTooLong::as_binprot_err(MAX_LENGTH, len));
        }

        Ok(Self(maybe_read_in_chunks(len, r)?, PhantomData))
    }
}

impl<const MAX_LENGTH: usize> binprot::BinProtWrite for BoundedByteString<MAX_LENGTH> {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        if self.0.len() > MAX_LENGTH {
            return Err(MinaStringTooLong::as_io_err(MAX_LENGTH, self.0.len()));
        }
        Nat0(self.0.len() as u64).binprot_write(w)?;
        w.write_all(&self.0)?;
        Ok(())
    }
}

/// Human-readable string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct BoundedCharString<const MAX_LENGTH: usize>(Vec<u8>, PhantomData<[u8; MAX_LENGTH]>);

impl<const MAX_LENGTH: usize> std::fmt::Debug for BoundedCharString<MAX_LENGTH> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(inner, _) = self;
        // Avoid vertical alignment
        f.write_fmt(format_args!("BoundedCharString({:?})", inner))
    }
}

impl<const MAX_LENGTH: usize> BoundedCharString<MAX_LENGTH> {
    pub fn to_string_lossy(&self) -> std::string::String {
        std::string::String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl<const MAX_LENGTH: usize> AsRef<[u8]> for BoundedCharString<MAX_LENGTH> {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl<const MAX_LENGTH: usize> From<Vec<u8>> for BoundedCharString<MAX_LENGTH> {
    fn from(source: Vec<u8>) -> Self {
        Self(source, PhantomData)
    }
}

impl<const MAX_LENGTH: usize> From<&[u8]> for BoundedCharString<MAX_LENGTH> {
    fn from(source: &[u8]) -> Self {
        Self(source.to_vec(), PhantomData)
    }
}

impl<const MAX_LENGTH: usize> From<&str> for BoundedCharString<MAX_LENGTH> {
    fn from(source: &str) -> Self {
        Self(source.as_bytes().to_vec(), PhantomData)
    }
}

impl<const MAX_LENGTH: usize> TryFrom<BoundedCharString<MAX_LENGTH>> for String {
    type Error = std::string::FromUtf8Error;

    fn try_from(value: BoundedCharString<MAX_LENGTH>) -> Result<Self, Self::Error> {
        String::from_utf8(value.0)
    }
}

impl<const MAX_LENGTH: usize> TryFrom<&BoundedCharString<MAX_LENGTH>> for String {
    type Error = std::string::FromUtf8Error;

    fn try_from(value: &BoundedCharString<MAX_LENGTH>) -> Result<Self, Self::Error> {
        String::from_utf8(value.0.clone())
    }
}

impl<const MAX_LENGTH: usize> PartialEq<[u8]> for BoundedCharString<MAX_LENGTH> {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_ref() == other
    }
}

impl<const MAX_LENGTH: usize> PartialEq<str> for BoundedCharString<MAX_LENGTH> {
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other.as_bytes()
    }
}

impl<const MAX_LENGTH: usize> std::fmt::Display for BoundedCharString<MAX_LENGTH> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_string_lossy())
    }
}

impl<const MAX_LENGTH: usize> binprot::BinProtRead for BoundedCharString<MAX_LENGTH> {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        let len = Nat0::binprot_read(r)?.0 as usize;
        if len > MAX_LENGTH {
            return Err(MinaStringTooLong::as_binprot_err(MAX_LENGTH, len));
        }

        Ok(Self(maybe_read_in_chunks(len, r)?, PhantomData))
    }
}

impl<const MAX_LENGTH: usize> binprot::BinProtWrite for BoundedCharString<MAX_LENGTH> {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        if self.0.len() > MAX_LENGTH {
            return Err(MinaStringTooLong::as_io_err(MAX_LENGTH, self.0.len()));
        }
        Nat0(self.0.len() as u64).binprot_write(w)?;
        w.write_all(&self.0)?;
        Ok(())
    }
}

/// Reads data from the reader `r` in chunks if the length `len` exceeds a predefined chunk size.
///
/// This approach avoids preallocating a large buffer upfront, which is crucial for handling
/// potentially large or untrusted input sizes efficiently and safely.
fn maybe_read_in_chunks<R: std::io::Read + ?Sized>(
    len: usize,
    r: &mut R,
) -> Result<Vec<u8>, binprot::Error> {
    if len <= CHUNK_SIZE {
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf)?;
        Ok(buf)
    } else {
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut temp_buf = vec![0u8; CHUNK_SIZE];
        let mut remaining = len;
        while remaining > 0 {
            let read_size = std::cmp::min(CHUNK_SIZE, remaining);
            r.read_exact(&mut temp_buf[..read_size])?;
            buf.extend_from_slice(&temp_buf[..read_size]);
            remaining -= read_size;
        }
        Ok(buf)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("String length `{actual}` is greater than maximum `{max}`")]
pub struct MinaStringTooLong {
    max: usize,
    actual: usize,
}

impl MinaStringTooLong {
    fn boxed(max: usize, actual: usize) -> Box<Self> {
        Box::new(MinaStringTooLong { max, actual })
    }

    fn as_io_err(max: usize, actual: usize) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            MinaStringTooLong::boxed(max, actual),
        )
    }

    fn as_binprot_err(max: usize, actual: usize) -> binprot::Error {
        binprot::Error::CustomError(MinaStringTooLong::boxed(max, actual))
    }
}
