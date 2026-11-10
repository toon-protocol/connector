use ark_ff::{BigInteger256, Field};
use mina_curves::pasta::Fp;

use crate::poseidon::{PlonkSpongeConstantsKimchi, Sponge, SpongeParamsForField};

/// The hash input of a Mina object: whole field elements, and bits that are packed into as few
/// field elements as fit (254 bits each, the `255 - 1` check in [`Inputs::to_fields`]).
pub struct Inputs {
    fields: Vec<Fp>,
    packeds: Vec<bool>,
}

impl Default for Inputs {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::needless_range_loop)]
fn shl(bigint: &mut [u64; 4], mut n: u32) {
    if n >= 64 * 4 {
        *bigint = [0, 0, 0, 0];
        return;
    }
    while n >= 64 {
        let mut t = 0;
        for i in 0..4 {
            core::mem::swap(&mut t, &mut bigint[i]);
        }
        n -= 64;
    }
    if n > 0 {
        let mut t = 0;
        for i in 0..4 {
            let a = &mut bigint[i];
            let t2 = *a >> (64 - n);
            *a <<= n;
            *a |= t;
            t = t2;
        }
    }
}

impl Inputs {
    pub fn new() -> Self {
        Self {
            fields: Vec::with_capacity(256),
            packeds: Vec::with_capacity(256),
        }
    }

    pub fn append_bool(&mut self, value: bool) {
        self.packeds.push(value);
    }

    pub fn append_field(&mut self, value: Fp) {
        self.fields.push(value);
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_fields(mut self) -> Vec<Fp> {
        let mut nbits = 0;
        let mut current: [u64; 4] = [0, 0, 0, 0];

        for item in self.packeds.iter().map(|&bit| bit as u64) {
            nbits += 1;

            if nbits < 255 {
                shl(&mut current, 1);

                // Addition, but we use 'bitwise or' because we know bits of
                // `current` are zero (we just shift-left them)
                current[0] |= item;
            } else {
                self.fields.push(BigInteger256::new(current).into()); // Never fail
                current = [item, 0, 0, 0];
                nbits = 1;
            }
        }

        if nbits > 0 {
            self.fields.push(BigInteger256::new(current).into()); // Never fail
        }

        self.fields
    }
}

fn param_to_field_impl(param: &str, default: &[u8; 32]) -> Fp {
    let param_bytes = param.as_bytes();
    let len = param_bytes.len();

    let mut fp = *default;
    fp[..len].copy_from_slice(param_bytes);

    Fp::from_random_bytes(&fp).expect("Must be a valid field element")
}

pub fn param_to_field(param: &str) -> Fp {
    const DEFAULT: &[u8; 32] = b"********************\0\0\0\0\0\0\0\0\0\0\0\0";

    if param.len() > 20 {
        panic!("must be 20 byte maximum");
    }

    param_to_field_impl(param, DEFAULT)
}

pub fn hash_with_kimchi(param: &LazyParam, fields: &[Fp]) -> Fp {
    let LazyParam {
        sponge_state,
        state,
        ..
    } = param;

    let mut sponge = Sponge {
        sponge_state: sponge_state.clone(),
        state: *state,
        ..Sponge::<Fp, PlonkSpongeConstantsKimchi>::default()
    };

    sponge.absorb(fields);
    sponge.squeeze()
}

pub fn hash_fields<F: Field + SpongeParamsForField<F>>(fields: &[F]) -> F {
    let mut sponge = Sponge::<F>::default();

    sponge.absorb(fields);
    sponge.squeeze()
}

#[derive(Debug)]
pub struct LazyParam {
    sponge_state: crate::poseidon::SpongeState,
    state: [Fp; 3],
}

/// The Poseidon sponge state after absorbing the domain-separation string of the side-loaded
/// verification key hash, `"MinaSideLoadedVk"`.
pub mod params {
    use std::sync::LazyLock;

    use super::*;

    pub static MINA_SIDELOADED_VK: LazyLock<Box<LazyParam>> = LazyLock::new(|| {
        let mut sponge = Sponge::<Fp>::default();
        sponge.absorb(&[param_to_field("MinaSideLoadedVk")]);
        // The parameter is the state *after* a squeeze: squeezing permutes it.
        sponge.squeeze();
        Box::new(LazyParam {
            sponge_state: sponge.sponge_state,
            state: sponge.state,
        })
    });
}
