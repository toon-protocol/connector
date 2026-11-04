#[derive(
    Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Deref,
)]
pub struct Number<T>(pub T);

impl<T: std::fmt::Debug> std::fmt::Debug for Number<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid vertical alignment
        f.write_fmt(format_args!("Number({inner:?})", inner = self.0))
    }
}

pub type UInt64 = Number<u64>;

impl UInt64 {
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u32> for Number<i32> {
    fn from(value: u32) -> Self {
        Self(value as i32)
    }
}

impl From<u64> for Number<i64> {
    fn from(value: u64) -> Self {
        Self(value as i64)
    }
}

impl From<&u32> for Number<i32> {
    fn from(value: &u32) -> Self {
        Self(*value as i32)
    }
}

impl From<&u64> for Number<i64> {
    fn from(value: &u64) -> Self {
        Self(*value as i64)
    }
}

impl From<&u32> for Number<u32> {
    fn from(value: &u32) -> Self {
        Self(*value)
    }
}

impl From<&u64> for Number<u64> {
    fn from(value: &u64) -> Self {
        Self(*value)
    }
}

macro_rules! binprot_number {
    ($base_type:ident, $binprot_type:ident) => {
        impl binprot::BinProtRead for Number<$base_type> {
            fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
            where
                Self: Sized,
            {
                $binprot_type::binprot_read(r).map(|v| Self(v as $base_type))
            }
        }

        impl binprot::BinProtWrite for Number<$base_type> {
            fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
                (self.0 as $binprot_type).binprot_write(w)
            }
        }
    };
}

binprot_number!(i32, i32);
binprot_number!(i64, i64);
binprot_number!(u32, i32);
binprot_number!(u64, i64);
binprot_number!(f64, f64);
