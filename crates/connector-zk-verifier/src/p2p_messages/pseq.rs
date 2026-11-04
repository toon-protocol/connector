use std::array;

use binprot::{BinProtRead, BinProtWrite};
use rsexp::{OfSexp, SexpOf};
#[derive(Clone, Debug, PartialEq)]
pub struct PaddedSeq<T, const N: usize>(pub [T; N]);

impl<T, const N: usize> Default for PaddedSeq<T, N>
where
    T: Default,
{
    fn default() -> Self {
        Self(array::from_fn(|_| T::default()))
    }
}

impl<T, const N: usize> std::ops::Deref for PaddedSeq<T, N> {
    type Target = [T; N];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: OfSexp, const N: usize> OfSexp for PaddedSeq<T, N> {
    fn of_sexp(s: &rsexp::Sexp) -> Result<Self, rsexp::IntoSexpError>
    where
        Self: Sized,
    {
        let elts = s.extract_list("PaddedSeq")?;
        if elts.len() != N {
            return Err(rsexp::IntoSexpError::ListLengthMismatch {
                type_: "PaddedSeq",
                expected_len: N,
                list_len: elts.len(),
            });
        }

        let mut converted: [Option<T>; N] = [(); N].map(|_| None);

        for (i, item) in elts.iter().enumerate() {
            converted[i] = Some(T::of_sexp(item)?);
        }

        // Unwrap cannot fail, otherwise we wouldn't have rechead this point
        Ok(Self(converted.map(|item| item.unwrap())))
    }
}

impl<T: SexpOf, const N: usize> rsexp::SexpOf for PaddedSeq<T, N> {
    fn sexp_of(&self) -> rsexp::Sexp {
        let elements: Vec<rsexp::Sexp> = self.0.iter().map(|item| item.sexp_of()).collect();

        rsexp::Sexp::List(elements)
    }
}

impl<T: BinProtRead, const N: usize> binprot::BinProtRead for PaddedSeq<T, N> {
    fn binprot_read<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        let mut vec = Vec::with_capacity(N);
        for _i in 0..N {
            vec.push(BinProtRead::binprot_read(r)?);
        }
        let _: () = BinProtRead::binprot_read(r)?;
        match vec.try_into() {
            Ok(arr) => Ok(PaddedSeq(arr)),
            Err(_) => unreachable!(),
        }
    }
}

impl<T: BinProtWrite, const N: usize> binprot::BinProtWrite for PaddedSeq<T, N> {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        for elt in &self.0 {
            elt.binprot_write(w)?;
        }
        ().binprot_write(w)?;
        Ok(())
    }
}
