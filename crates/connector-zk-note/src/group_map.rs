//! `hashToGroup`, as o1js's `Poseidon.hashToGroup`: Poseidon, then Snarky's conic group map,
//! with `y` negated when it is odd.
//!
//! proof-systems' `groupmap` crate is a **different** map (BW19, the one the IPA uses) and
//! does not match o1js (spike #1512 §2), so this ports the map mina-rust carries in
//! `crates/ledger/src/proofs/group_map.rs` (`tock` parameters, `to_group`) instead of
//! depending on either.
//!
//! The port is of the algorithm of <https://github.com/o1-labs/mina-rust> `v0.19.0`
//! (`ed6f4301`), which is Apache-2.0, Copyright o1Labs. This file is a derivative work in
//! that sense and carries that licence's attribution; the changes are that the circuit
//! witness plumbing is dropped, leaving the plain field arithmetic.

use crate::poseidon;
use crate::{Field, Point};
use ark_ec::short_weierstrass::SWCurveConfig;
use ark_ff::{BigInteger, Field as _, One, PrimeField, Zero};
use mina_curves::pasta::PallasParameters;

fn is_square(x: Field) -> bool {
    x.is_zero() || x.legendre().is_qr()
}

fn first<T>(f: impl Fn(Field) -> Option<T>) -> T {
    let mut v = Field::zero();
    loop {
        if let Some(x) = f(v) {
            return x;
        }
        v += Field::one();
    }
}

struct Params {
    u: Field,
    u_over_2: Field,
    conic_c: Field,
    z0: Field,
    y0: Field,
}

fn params() -> Params {
    let a = PallasParameters::COEFF_A;
    let b = PallasParameters::COEFF_B;
    let three_fourths = Field::from(3u64) / Field::from(4u64);
    let curve_eqn = |u: Field| u * u * u + a * u + b;

    let u = first(|u| {
        let check = three_fourths * u * u + a;
        let fu = curve_eqn(u);
        (!check.is_zero() && !fu.is_zero() && !is_square(-fu)).then_some(u)
    });
    let conic_c = three_fourths * u * u + a;
    let conic_d = -curve_eqn(u);
    let (z0, y0) = first(|y| {
        let z2 = conic_d - conic_c * y * y;
        is_square(z2).then(|| (z2.sqrt().expect("checked square"), y))
    });
    Params {
        u,
        u_over_2: u / Field::from(2u64),
        conic_c,
        z0,
        y0,
    }
}

/// The Snarky conic map of a field element to a point of Pallas.
fn to_group(t: Field) -> Point {
    let p = params();
    let one = Field::one();

    // field_to_conic
    let ct = p.conic_c * t;
    let s = Field::from(2u64) * (ct * p.y0 + p.z0) / (ct * t + one);
    let (z, y) = (p.z0 - s, p.y0 - s * t);

    // conic_to_s, s_to_v_truncated
    let v = z * y.inverse().unwrap_or_else(Field::zero) - p.u_over_2;
    let candidates = [v, -(p.u + v), p.u + y * y];

    // wrap: the first candidate x whose curve equation is a square
    let rhs = |x: Field| x * x * x + PallasParameters::COEFF_A * x + PallasParameters::COEFF_B;
    let x = candidates
        .into_iter()
        .find(|&x| is_square(rhs(x)))
        .expect("one of the three candidates is always on the curve");
    let y = rhs(x).sqrt().expect("checked square");
    if x.is_zero() && y.is_zero() {
        <Point as ark_ec::AffineRepr>::zero()
    } else {
        Point::new_unchecked(x, y)
    }
}

/// `Poseidon.hashToGroup(input)`: the point whose `y` is even.
pub fn hash_to_group(input: &[Field]) -> Point {
    let p = to_group(poseidon::hash(input));
    if p.y.into_bigint().is_odd() {
        Point::new_unchecked(p.x, -p.y)
    } else {
        p
    }
}
