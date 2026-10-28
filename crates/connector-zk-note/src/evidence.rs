//! The evidence file's bytes: the append-only record of every allowance granted and how each
//! closed, which zk_toon's challenger reads (ADR 0080 decisions 11 and 12).
//!
//! **Stand-in for zk_toon's `zk-evidence` crate.** The format is zk_toon's, decided at triage in
//! toon-protocol/zk_toon#29 and zk_toon's ADR 0004, but the crate that will encode it does not
//! exist at the pinned commit. This module writes exactly the bytes that brief specifies, and
//! nothing else in the connector knows them, so when `zk-evidence` lands this module becomes a
//! re-export of it and `tests/evidence_format.rs` becomes a replay of its vectors. One point the
//! brief leaves open is settled here and must be checked against zk_toon's crate then: the
//! `JsonProof`'s length prefix is a `u32` LE.
//!
//! - **File:** `TOONZKEV`, a `u16` LE version (`1`), then records.
//! - **Framing:** `u32 LE payload length ‖ payload ‖ CRC32C(payload)` (LE). A short or CRC-bad
//!   record at the end is a torn write, ignored by a reader and cut off by a writer; a CRC
//!   failure anywhere else is corruption.
//! - **Evidence hash:** SHA-256 of the payload alone. The journal stores it.
//! - **Payload:** a kind byte (`1` grant, `2` close), then the fields below. Field elements are
//!   32 bytes LE and canonical; amounts and times are `u64` LE; the y-is-odd flag is one byte;
//!   the allowance id is a `u8` length and at most 64 opaque bytes.
//!
//! No I/O: the connector's file writer frames with [`frame`] and recovers with [`scan`].

use crate::{field_from_le_bytes, field_to_le_bytes, CompressedPubKey, Field};
use sha2::{Digest, Sha256};

/// The file's first eight bytes.
pub const MAGIC: [u8; 8] = *b"TOONZKEV";
/// The only format version this build reads or writes.
pub const VERSION: u16 = 1;
/// The longest allowance id a record may carry.
pub const MAX_ALLOWANCE_ID_LEN: usize = 64;

const KIND_GRANT: u8 = 1;
const KIND_CLOSE: u8 = 2;
const HEADER_LEN: usize = 10;

/// What a grant record holds: written and fsynced before the allowance is granted.
///
/// Tier, nullifier and the subsidised flag are not repeated: they are in the proof's public
/// input. No secret is ever in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRecord {
    /// The request proof, o1js `JsonProof` bytes, verbatim.
    pub proof_json: Vec<u8>,
    /// The allowance id the zk edge minted.
    pub allowance_id: Vec<u8>,
    /// The session public key.
    pub session_key: CompressedPubKey,
    /// The allowance's expiry, ms.
    pub expiry: u64,
    /// When the zk edge granted it, ms.
    pub granted_at: u64,
}

/// What a close record holds: written and fsynced before the signed successor is released.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseRecord {
    /// The nullifier the allowance spent.
    pub nullifier: Field,
    /// The allowance id.
    pub allowance_id: Vec<u8>,
    /// The charge Δ, nanomina.
    pub charge: u64,
}

/// One record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// An allowance granted.
    Grant(GrantRecord),
    /// An allowance closed.
    Close(CloseRecord),
}

/// Why evidence bytes are refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    /// The file does not start with [`MAGIC`].
    #[error("the evidence file does not start with its magic")]
    BadMagic,
    /// A format version this build does not know.
    #[error("the evidence file is format version {0}; this build reads only version {VERSION}")]
    UnknownVersion(u16),
    /// A record before the end failed its CRC: not a torn write, damage.
    #[error("the evidence file is corrupt at byte {offset}")]
    Corrupt {
        /// Where the bad record starts.
        offset: usize,
    },
    /// A payload kind other than grant or close.
    #[error("an evidence record of unknown kind {0}")]
    UnknownKind(u8),
    /// A payload shorter than its kind requires, or longer.
    #[error("an evidence record is not the length its kind requires")]
    BadLength,
    /// A field element at or above the modulus.
    #[error("an evidence record holds a non-canonical field element")]
    NonCanonicalField,
    /// A flag byte other than 0 or 1.
    #[error("an evidence record holds a flag that is neither 0 nor 1")]
    NotABool,
    /// An allowance id longer than [`MAX_ALLOWANCE_ID_LEN`].
    #[error("an allowance id of {0} bytes is longer than {MAX_ALLOWANCE_ID_LEN}")]
    AllowanceIdTooLong(usize),
    /// A `JsonProof` too long for its length prefix.
    #[error("a request proof too long to frame")]
    ProofTooLong,
}

/// The file header: magic and version.
#[must_use]
pub fn header() -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[..8].copy_from_slice(&MAGIC);
    out[8..].copy_from_slice(&VERSION.to_le_bytes());
    out
}

impl Record {
    /// The record's payload bytes.
    ///
    /// # Errors
    /// [`EvidenceError::AllowanceIdTooLong`] or [`EvidenceError::ProofTooLong`].
    pub fn to_payload(&self) -> Result<Vec<u8>, EvidenceError> {
        let mut out = Vec::new();
        match self {
            Record::Grant(grant) => {
                out.push(KIND_GRANT);
                let len = u32::try_from(grant.proof_json.len())
                    .map_err(|_| EvidenceError::ProofTooLong)?;
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&grant.proof_json);
                put_id(&mut out, &grant.allowance_id)?;
                out.extend_from_slice(&field_to_le_bytes(grant.session_key.x));
                out.push(u8::from(grant.session_key.is_odd));
                out.extend_from_slice(&grant.expiry.to_le_bytes());
                out.extend_from_slice(&grant.granted_at.to_le_bytes());
            }
            Record::Close(close) => {
                out.push(KIND_CLOSE);
                out.extend_from_slice(&field_to_le_bytes(close.nullifier));
                put_id(&mut out, &close.allowance_id)?;
                out.extend_from_slice(&close.charge.to_le_bytes());
            }
        }
        Ok(out)
    }

    /// Read a record's payload bytes.
    ///
    /// # Errors
    /// [`EvidenceError`] naming what the bytes are not.
    pub fn from_payload(payload: &[u8]) -> Result<Record, EvidenceError> {
        let mut r = Reader(payload);
        let record = match r.u8()? {
            KIND_GRANT => {
                let len = r.u32()? as usize;
                let proof_json = r.take(len)?.to_vec();
                let allowance_id = r.id()?;
                let x = r.field()?;
                let is_odd = r.bool()?;
                Record::Grant(GrantRecord {
                    proof_json,
                    allowance_id,
                    session_key: CompressedPubKey { x, is_odd },
                    expiry: r.u64()?,
                    granted_at: r.u64()?,
                })
            }
            KIND_CLOSE => Record::Close(CloseRecord {
                nullifier: r.field()?,
                allowance_id: r.id()?,
                charge: r.u64()?,
            }),
            other => return Err(EvidenceError::UnknownKind(other)),
        };
        if !r.0.is_empty() {
            return Err(EvidenceError::BadLength);
        }
        Ok(record)
    }
}

/// A payload, framed for appending: length, payload, CRC32C.
#[must_use]
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("an evidence payload fits 32 bits");
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32c(payload).to_le_bytes());
    out
}

/// The evidence hash: SHA-256 of the payload alone.
#[must_use]
pub fn evidence_hash(payload: &[u8]) -> [u8; 32] {
    Sha256::digest(payload).into()
}

/// What [`scan`] found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    /// Every whole record's payload, in file order.
    pub records: Vec<Vec<u8>>,
    /// The length of the file's sound prefix: where a writer truncates to before appending. Zero
    /// when even the header is incomplete, so the writer writes it.
    pub end: usize,
    /// Whether anything followed [`Self::end`]: a torn final write.
    pub torn: bool,
}

/// Read a whole file's bytes.
///
/// # Errors
/// [`EvidenceError::BadMagic`], [`EvidenceError::UnknownVersion`], or
/// [`EvidenceError::Corrupt`] for a CRC failure that is not the file's last record.
pub fn scan(file: &[u8]) -> Result<Scan, EvidenceError> {
    if file.len() < HEADER_LEN {
        if header()[..file.len()] != *file {
            return Err(EvidenceError::BadMagic);
        }
        return Ok(Scan {
            records: Vec::new(),
            end: 0,
            torn: !file.is_empty(),
        });
    }
    if file[..8] != MAGIC {
        return Err(EvidenceError::BadMagic);
    }
    let version = u16::from_le_bytes([file[8], file[9]]);
    if version != VERSION {
        return Err(EvidenceError::UnknownVersion(version));
    }
    let mut records = Vec::new();
    let mut at = HEADER_LEN;
    while at < file.len() {
        let rest = &file[at..];
        let torn = || Scan {
            records: Vec::new(),
            end: at,
            torn: true,
        };
        if rest.len() < 4 {
            return Ok(Scan { records, ..torn() });
        }
        let len = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let Some(whole) = len.checked_add(8).filter(|whole| *whole <= rest.len()) else {
            return Ok(Scan { records, ..torn() });
        };
        let payload = &rest[4..4 + len];
        let crc = u32::from_le_bytes([rest[4 + len], rest[5 + len], rest[6 + len], rest[7 + len]]);
        if crc != crc32c(payload) {
            if whole == rest.len() {
                return Ok(Scan { records, ..torn() });
            }
            return Err(EvidenceError::Corrupt { offset: at });
        }
        records.push(payload.to_vec());
        at += whole;
    }
    Ok(Scan {
        records,
        end: at,
        torn: false,
    })
}

/// CRC-32C (Castagnoli, reflected polynomial `0x82F63B78`), as the framing requires.
#[must_use]
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    !crc
}

fn put_id(out: &mut Vec<u8>, id: &[u8]) -> Result<(), EvidenceError> {
    if id.len() > MAX_ALLOWANCE_ID_LEN {
        return Err(EvidenceError::AllowanceIdTooLong(id.len()));
    }
    out.push(id.len() as u8);
    out.extend_from_slice(id);
    Ok(())
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], EvidenceError> {
        if self.0.len() < n {
            return Err(EvidenceError::BadLength);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, EvidenceError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, EvidenceError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, EvidenceError> {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }

    fn bool(&mut self) -> Result<bool, EvidenceError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(EvidenceError::NotABool),
        }
    }

    fn field(&mut self) -> Result<Field, EvidenceError> {
        let mut b = [0u8; 32];
        b.copy_from_slice(self.take(32)?);
        field_from_le_bytes(&b).ok_or(EvidenceError::NonCanonicalField)
    }

    fn id(&mut self) -> Result<Vec<u8>, EvidenceError> {
        let len = self.u8()? as usize;
        if len > MAX_ALLOWANCE_ID_LEN {
            return Err(EvidenceError::AllowanceIdTooLong(len));
        }
        Ok(self.take(len)?.to_vec())
    }
}
