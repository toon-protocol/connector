use binprot::byteorder::{ReadBytesExt, WriteBytesExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Char(pub u8);

impl std::ops::Deref for Char {
    type Target = u8;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Char {
    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

impl From<u8> for Char {
    fn from(source: u8) -> Self {
        Self(source)
    }
}

impl From<Char> for u8 {
    fn from(source: Char) -> Self {
        source.0
    }
}

impl binprot::BinProtRead for Char {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        Ok(r.read_u8().map(Self)?)
    }
}

impl binprot::BinProtWrite for Char {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_u8(self.0)
    }
}
