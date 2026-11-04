use ark_ff::{BigInteger256, PrimeField};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use rsexp::{OfSexp, SexpOf};

// ---
// This has been imported from a fork of arkworks/ff
// We should probably revisit this structure in the future
#[derive(Clone, Debug)]
pub struct InvalidBigInt;

impl core::fmt::Display for InvalidBigInt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "InvalidBigInt")
    }
}

impl From<InvalidBigInt> for String {
    fn from(_: InvalidBigInt) -> Self {
        "InvalidBigInt".to_string()
    }
}

impl std::error::Error for InvalidBigInt {}
// ---

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
pub struct BigInt(BigInteger256);

impl std::fmt::Debug for BigInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(bigint) = self;
        // Avoid vertical alignment
        f.write_fmt(format_args!("BigInt({:?})", bigint.0))
    }
}

impl BigInt {
    pub fn zero() -> Self {
        mina_curves::pasta::Fp::from(0u64).into()
    }

    pub fn one() -> Self {
        mina_curves::pasta::Fp::from(1u64).into()
    }

    pub fn to_field<F>(&self) -> Result<F, InvalidBigInt>
    where
        F: ark_ff::Field + From<BigInteger256>,
    {
        let Self(biginteger) = self;
        Ok(F::from(*biginteger))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        let mut bytes = Vec::with_capacity(32);
        self.0.serialize_uncompressed(&mut bytes).unwrap(); // Never fail, there is 32 bytes
        bytes.try_into().unwrap()
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        let value = BigInteger256::deserialize_uncompressed(&bytes[..]).expect("Don't fail");
        Self(value) // Never fail, we read from 32 bytes
    }
}

impl AsRef<BigInteger256> for BigInt {
    fn as_ref(&self) -> &BigInteger256 {
        let Self(biginteger) = self;
        biginteger
    }
}

impl From<mina_curves::pasta::Fp> for BigInt {
    fn from(field: mina_curves::pasta::Fp) -> Self {
        Self(field.into_bigint())
    }
}

impl From<mina_curves::pasta::Fq> for BigInt {
    fn from(field: mina_curves::pasta::Fq) -> Self {
        Self(field.into_bigint())
    }
}

impl From<&mina_curves::pasta::Fp> for BigInt {
    fn from(field: &mina_curves::pasta::Fp) -> Self {
        Self(field.into_bigint())
    }
}

impl From<&mina_curves::pasta::Fq> for BigInt {
    fn from(field: &mina_curves::pasta::Fq) -> Self {
        Self(field.into_bigint())
    }
}

impl TryFrom<BigInt> for mina_curves::pasta::Fp {
    type Error = InvalidBigInt;

    fn try_from(bigint: BigInt) -> Result<Self, Self::Error> {
        bigint.to_field()
    }
}

impl TryFrom<BigInt> for mina_curves::pasta::Fq {
    type Error = InvalidBigInt;

    fn try_from(bigint: BigInt) -> Result<Self, Self::Error> {
        bigint.to_field()
    }
}

impl TryFrom<&BigInt> for mina_curves::pasta::Fp {
    type Error = InvalidBigInt;

    fn try_from(bigint: &BigInt) -> Result<Self, Self::Error> {
        bigint.to_field()
    }
}

impl TryFrom<&BigInt> for mina_curves::pasta::Fq {
    type Error = InvalidBigInt;

    fn try_from(bigint: &BigInt) -> Result<Self, Self::Error> {
        bigint.to_field()
    }
}

impl OfSexp for BigInt {
    fn of_sexp(s: &rsexp::Sexp) -> Result<Self, rsexp::IntoSexpError>
    where
        Self: Sized,
    {
        let bytes = s.extract_atom("BigInt")?;
        let hex_str = std::str::from_utf8(bytes).map_err(|_| {
            rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected hex string with 0x prefix, got {bytes:?}"),
            }
        })?;

        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

        let padded_hex = format!("{:0>64}", hex_str);

        if padded_hex.len() != 64 {
            return Err(rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected 64-character hex string, got {padded_hex:?}"),
            });
        }

        let byte_vec: Vec<u8> = (0..padded_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&padded_hex[i..i + 2], 16))
            .rev()
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| rsexp::IntoSexpError::StringConversionError {
                err: format!("Failed to parse hex string: {padded_hex:?}"),
            })?;

        Ok(BigInt::from_bytes(byte_vec.try_into().unwrap()))
    }
}

impl SexpOf for BigInt {
    fn sexp_of(&self) -> rsexp::Sexp {
        use std::fmt::Write;
        let byte_vec = self.to_bytes();
        let hex_str = byte_vec
            .iter()
            .rev()
            .fold("0x".to_string(), |mut output, byte| {
                let _ = write!(output, "{byte:02X}");
                output
            });

        rsexp::Sexp::Atom(hex_str.into_bytes())
    }
}

impl binprot::BinProtRead for BigInt {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        let mut bytes = [0u8; 32];
        r.read_exact(&mut bytes)?;
        Ok(Self::from_bytes(bytes))
    }
}

impl binprot::BinProtWrite for BigInt {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&self.to_bytes())
    }
}

impl mina_hasher::Hashable for BigInt {
    type D = ();

    fn to_roinput(&self) -> mina_hasher::ROInput {
        mina_hasher::ROInput::new()
            .append_field(self.to_field().expect("Failed to convert Hash into Fp"))
    }

    fn domain_string(_: Self::D) -> Option<String> {
        None
    }
}
