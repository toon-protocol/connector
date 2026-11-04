use super::*;

//
//  Location: [src/lib/parallel_scan/parallel_scan.ml:247:6](https://github.com/openmina/mina/blob/da4c511501876adff40f3e1281392fedd121d607/src/lib/parallel_scan/parallel_scan.ml#L247)
//
//  Gid: 947

// TODO: many of these OfSexp/SexpOf implementations can be removed if rsexp-derive is forked and modified
// to fix a big in how enums are handled, and to avoid intermediary wrapping types in the output

impl rsexp::OfSexp for PicklesBaseProofsVerifiedStableV1 {
    fn of_sexp(s: &rsexp::Sexp) -> Result<Self, rsexp::IntoSexpError>
    where
        Self: Sized,
    {
        match s.extract_enum("PicklesBaseProofsVerifiedStableV1")? {
            (b"N0", _) => Ok(PicklesBaseProofsVerifiedStableV1::N0),
            (b"N1", _) => Ok(PicklesBaseProofsVerifiedStableV1::N1),
            (b"N2", _) => Ok(PicklesBaseProofsVerifiedStableV1::N2),
            (ctor, _) => Err(rsexp::IntoSexpError::UnknownConstructorForEnum {
                type_: "PicklesBaseProofsVerifiedStableV1",
                constructor: String::from_utf8_lossy(ctor).to_string(),
            }),
        }
    }
}

impl rsexp::OfSexp for LimbVectorConstantHex64StableV1 {
    fn of_sexp(s: &rsexp::Sexp) -> Result<Self, rsexp::IntoSexpError>
    where
        Self: Sized,
    {
        let bytes = s.extract_atom("LimbVectorConstantHex64StableV1")?;
        let hex_str = std::str::from_utf8(bytes).map_err(|_| {
            rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected 16 bytes hex string, got {bytes:?}"),
            }
        })?;
        if hex_str.len() != 16 {
            return Err(rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected 16 bytes hex string, got {hex_str:?}"),
            });
        }
        let n = u64::from_str_radix(hex_str, 16).map_err(|_| {
            rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected 16 bytes hex string, got {hex_str:?}"),
            }
        })?;

        Ok(Self(n.into()))
    }
}

impl rsexp::SexpOf for LimbVectorConstantHex64StableV1 {
    fn sexp_of(&self) -> rsexp::Sexp {
        let value: u64 = self.0.as_u64();
        let hex_str = format!("{:016x}", value);

        rsexp::Sexp::Atom(format!("0x{}", hex_str).into_bytes())
    }
}

impl rsexp::OfSexp for CompositionTypesBranchDataDomainLog2StableV1 {
    fn of_sexp(s: &rsexp::Sexp) -> Result<Self, rsexp::IntoSexpError>
    where
        Self: Sized,
    {
        match s.extract_atom("CompositionTypesBranchDataDomainLog2StableV1")? {
            [ch] => Ok(Self((*ch).into())),
            bytes => Err(rsexp::IntoSexpError::StringConversionError {
                err: format!("Expected single byte string, got {bytes:?}"),
            }),
        }
    }
}

impl rsexp::SexpOf for CompositionTypesBranchDataDomainLog2StableV1 {
    fn sexp_of(&self) -> rsexp::Sexp {
        rsexp::Sexp::Atom(vec![self.0.as_u8()])
    }
}

impl rsexp::OfSexp for CompositionTypesDigestConstantStableV1 {
    fn of_sexp(s: &rsexp::Sexp) -> std::result::Result<Self, rsexp::IntoSexpError> {
        Ok(Self(crate::p2p_messages::pseq::PaddedSeq::<
            LimbVectorConstantHex64StableV1,
            4,
        >::of_sexp(s)?))
    }
}

impl rsexp::SexpOf for CompositionTypesDigestConstantStableV1 {
    fn sexp_of(&self) -> rsexp::Sexp {
        self.0.sexp_of()
    }
}

impl rsexp::OfSexp for PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2 {
    fn of_sexp(s: &rsexp::Sexp) -> std::result::Result<Self, rsexp::IntoSexpError> {
        Ok(Self(crate::p2p_messages::pseq::PaddedSeq::<
            PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A,
            15,
        >::of_sexp(s)?))
    }
}

impl rsexp::SexpOf for PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2 {
    fn sexp_of(&self) -> rsexp::Sexp {
        self.0.sexp_of()
    }
}

impl From<&PicklesProofProofsVerifiedMaxStableV2> for PicklesProofProofsVerified2ReprStableV2 {
    fn from(value: &PicklesProofProofsVerifiedMaxStableV2) -> Self {
        let PicklesProofProofsVerifiedMaxStableV2 {
            statement,
            prev_evals,
            proof,
        } = value;

        Self {
            statement: statement.clone(),
            prev_evals: prev_evals.clone(),
            proof: proof.clone(),
        }
    }
}

// PicklesProofProofsVerifiedMaxStableV2 PicklesProofProofsVerified2ReprStableV2

impl From<PicklesProofProofsVerifiedMaxStableV2> for PicklesProofProofsVerified2ReprStableV2 {
    fn from(value: PicklesProofProofsVerifiedMaxStableV2) -> Self {
        Self {
            statement: value.statement,
            prev_evals: value.prev_evals,
            proof: value.proof,
        }
    }
}

impl From<PicklesProofProofsVerified2ReprStableV2> for PicklesProofProofsVerifiedMaxStableV2 {
    fn from(value: PicklesProofProofsVerified2ReprStableV2) -> Self {
        Self {
            statement: value.statement,
            prev_evals: value.prev_evals,
            proof: value.proof,
        }
    }
}
