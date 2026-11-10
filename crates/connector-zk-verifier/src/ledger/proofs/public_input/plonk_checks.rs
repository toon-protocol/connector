use std::rc::Rc;

use ark_ff::{Field, One};
use ark_poly::Radix2EvaluationDomain;
use kimchi::{
    circuits::expr::RowOffset,
    curve::KimchiCurve,
    proof::{PointEvaluations, ProofEvaluations},
};
use mina_curves::pasta::{Fp, Fq};

use crate::ledger::{
    lazy::LazyValue,
    proofs::{
        field::{field, Boolean, FieldWitness},
        public_input::plonk_checks::scalars::MinimalForScalar,
        step::step_verifier::PlonkDomain,
        to_field_elements::ToFieldElements,
        witness::Witness,
        wrap::AllFeatureFlags,
    },
};

#[derive(Clone, Debug)]
pub struct PlonkMinimal<F: FieldWitness, const NLIMB: usize = 2> {
    pub alpha: F,
    pub beta: F,
    pub gamma: F,
    pub zeta: F,
    pub joint_combiner: Option<F>,
    pub alpha_bytes: [u64; NLIMB],
    pub beta_bytes: [u64; NLIMB],
    pub gamma_bytes: [u64; NLIMB],
    pub zeta_bytes: [u64; NLIMB],
    pub joint_combiner_bytes: Option<[u64; NLIMB]>,
    pub feature_flags: crate::ledger::proofs::step::FeatureFlags<bool>,
}

pub struct ScalarsEnv<F: FieldWitness> {
    pub zk_polynomial: F,
    pub zeta_to_n_minus_1: F,
    pub domain: Rc<dyn PlonkDomain<F>>,
    pub omega_to_minus_zk_rows: F,
    pub feature_flags: Option<AllFeatureFlags<F>>,
    pub unnormalized_lagrange_basis: Option<Box<dyn Fn(RowOffset, &mut Witness<F>) -> F>>,
    pub vanishes_on_zero_knowledge_and_previous_rows: F,
    pub zeta_to_srs_length: LazyValue<F, Witness<F>>,
}

// Result of `plonk_derive`
#[derive(Debug)]
pub struct InCircuit<F: FieldWitness> {
    pub zeta_to_domain_size: F::Shifting,
    pub zeta_to_srs_length: F::Shifting,
    pub perm: F::Shifting,
    pub lookup: Option<F>,
    pub feature_flags: crate::ledger::proofs::step::FeatureFlags<bool>,
}

pub trait ShiftingValue<F: Field> {
    type MyShift;
    fn shift() -> Self::MyShift;
    fn of_field(field: F) -> Self;
}

impl ShiftingValue<Fp> for ShiftedValue<Fp> {
    type MyShift = Shift<Fp>;

    fn shift() -> Self::MyShift {
        type MyShift = Shift<Fp>;

        cache_one! {MyShift, {
            let c = (0..255).fold(Fp::one(), |accum, _| accum + accum) + Fp::one();

            let scale: Fp = 2.into();
            let scale = scale.inverse().unwrap();

            Shift { c, scale }
        }}
    }

    fn of_field(field: Fp) -> Self {
        let shift = Self::shift();
        Self {
            shifted: (field - shift.c) * shift.scale,
        }
    }
}

impl ShiftingValue<Fq> for ShiftedValue<Fq> {
    type MyShift = ShiftFq;

    fn shift() -> Self::MyShift {
        cache_one! {ShiftFq, {
            ShiftFq {
                shift: (0..255).fold(Fq::one(), |accum, _| accum + accum),
            }
        }}
    }

    fn of_field(field: Fq) -> Self {
        let shift = Self::shift();
        Self {
            shifted: field - shift.shift,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShiftFq {
    shift: Fq,
}

#[derive(Clone, Debug)]
pub struct Shift<F: Field> {
    c: F,
    scale: F,
}

impl<F> Shift<F> where F: Field + From<i32> {}

#[derive(Clone, Debug)]
pub struct ShiftedValue<F: Field> {
    pub shifted: F,
}

// impl<F: Field + FpExt> std::fmt::Debug for ShiftedValue<F> {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         f.debug_struct("ShiftedValue")
//             .field("shifted", &{
//                 let mut bytes = self.shifted.to_bytes();
//                 bytes.reverse();
//                 hex::encode(bytes)
//             })
//             .finish()
//     }
// }

impl<F: FieldWitness, F2: FieldWitness + ToFieldElements<F>> ToFieldElements<F>
    for ShiftedValue<F2>
{
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self { shifted } = self;
        shifted.to_field_elements(fields);
    }
}

impl<F> ShiftedValue<F>
where
    F: Field,
{
    // /// <https://github.com/MinaProtocol/mina/blob/0b63498e271575dbffe2b31f3ab8be293490b1ac/src/lib/pickles_types/shifted_value.ml#L127>
    // pub fn of_field(field: F, shift: &Shift<F>) -> Self {
    //     Self {
    //         shifted: (field - shift.c) * shift.scale,
    //     }
    // }

    // /// <https://github.com/MinaProtocol/mina/blob/0b63498e271575dbffe2b31f3ab8be293490b1ac/src/lib/pickles_types/shifted_value.ml#L131>
    // #[allow(unused)]
    // pub fn to_field(&self, shift: &Shift<F>) -> F {
    //     self.shifted + self.shifted + shift.c
    // }
}

/// <https://github.com/MinaProtocol/mina/blob/0b63498e271575dbffe2b31f3ab8be293490b1ac/src/lib/pickles/plonk_checks/plonk_checks.ml#L218>
pub const PERM_ALPHA0: usize = 21;

pub const NPOWERS_OF_ALPHA: usize = PERM_ALPHA0 + 3;

/// <https://github.com/MinaProtocol/mina/blob/0b63498e271575dbffe2b31f3ab8be293490b1ac/src/lib/pickles/plonk_checks/plonk_checks.ml#L141>
pub fn powers_of_alpha<F: FieldWitness>(alpha: F) -> Box<[F; NPOWERS_OF_ALPHA]> {
    // The OCaml code computes until alpha^71, but we don't need that much here
    let mut alphas = Box::new([F::one(); NPOWERS_OF_ALPHA]);

    alphas[1] = alpha;
    for i in 2..alphas.len() {
        alphas[i] = alpha * alphas[i - 1];
    }

    alphas
}

pub fn derive_plonk<F: FieldWitness, const NLIMB: usize>(
    env: &ScalarsEnv<F>,
    evals: &ProofEvaluations<PointEvaluations<F>>,
    minimal: &PlonkMinimal<F, NLIMB>,
) -> InCircuit<F> {
    let PlonkMinimal {
        alpha,
        beta,
        gamma,
        joint_combiner,
        feature_flags: actual_feature_flags,
        ..
    } = minimal;

    let zkp = env.zk_polynomial;
    let powers_of_alpha = powers_of_alpha(*alpha);
    let alpha_pow = |i: usize| powers_of_alpha[i];
    let w0 = evals.w.map(|point| point.zeta);

    let beta = *beta;
    let gamma = *gamma;

    // <https://github.com/MinaProtocol/mina/blob/0b63498e271575dbffe2b31f3ab8be293490b1ac/src/lib/pickles/plonk_checks/plonk_checks.ml#L397>
    let perm = evals.s.iter().enumerate().fold(
        evals.z.zeta_omega * beta * alpha_pow(PERM_ALPHA0) * zkp,
        |accum, (index, elem)| accum * (gamma + (beta * elem.zeta) + w0[index]),
    );
    let perm = -perm;

    let zeta_to_domain_size = env.zeta_to_n_minus_1 + F::one();

    let zeta_to_srs_length = {
        let mut unused_w = Witness::empty();
        *env.zeta_to_srs_length.get(&mut unused_w)
    };

    // Shift values
    let shift = |f: F| F::Shifting::of_field(f);

    InCircuit {
        zeta_to_domain_size: shift(zeta_to_domain_size),
        zeta_to_srs_length: shift(zeta_to_srs_length),
        perm: shift(perm),
        lookup: joint_combiner
            .as_ref()
            .map(|joint_combiner| *joint_combiner),
        feature_flags: actual_feature_flags.clone(),
    }
}

// TODO: De-duplicate with `derive_plonk`

pub fn make_shifts<F: FieldWitness>(
    domain: &Radix2EvaluationDomain<F>,
) -> kimchi::circuits::polynomials::permutation::Shifts<F> {
    // let value = 1 << log2_size;
    // let domain = Domain::<Fq>::new(value).unwrap();
    kimchi::circuits::polynomials::permutation::Shifts::new(domain)
}

pub fn ft_eval0<F: FieldWitness, const NLIMB: usize>(
    env: &ScalarsEnv<F>,
    evals: &ProofEvaluations<PointEvaluations<F>>,
    minimal: &PlonkMinimal<F, NLIMB>,
    p_eval0: &[F],
) -> F {
    const PLONK_TYPES_PERMUTS_MINUS_1_N: usize = 6;

    let e0_s: Vec<_> = evals.s.iter().map(|s| s.zeta).collect();
    let zkp = env.zk_polynomial;
    let powers_of_alpha = powers_of_alpha(minimal.alpha);
    let alpha_pow = |i: usize| powers_of_alpha[i];

    let zeta1m1 = env.zeta_to_n_minus_1;

    let mut unused_w = Witness::empty();
    let p_eval0 = p_eval0
        .iter()
        .copied()
        .rfold(None, |acc, p_eval0| match acc {
            None => Some(p_eval0),
            Some(acc) => {
                let zeta1 = *env.zeta_to_srs_length.get(&mut unused_w);
                Some(p_eval0 + (zeta1 * acc))
            }
        })
        .unwrap(); // Never fail, `p_eval0` is non-empty

    let w0: Vec<_> = evals.w.iter().map(|w| w.zeta).collect();

    let ft_eval0 = {
        let a0 = alpha_pow(PERM_ALPHA0);
        let w_n = w0[PLONK_TYPES_PERMUTS_MINUS_1_N];
        let init = (w_n + minimal.gamma) * evals.z.zeta_omega * a0 * zkp;
        e0_s.iter().enumerate().fold(init, |acc, (i, s)| {
            ((minimal.beta * s) + w0[i] + minimal.gamma) * acc
        })
    };

    let shifts = env.domain.shifts();
    let ft_eval0 = ft_eval0 - p_eval0;

    let ft_eval0 = ft_eval0
        - shifts.iter().enumerate().fold(
            alpha_pow(PERM_ALPHA0) * zkp * evals.z.zeta,
            |acc, (i, s)| acc * (minimal.gamma + (minimal.beta * minimal.zeta * s) + w0[i]),
        );

    let nominator =
        (zeta1m1 * alpha_pow(PERM_ALPHA0 + 1) * (minimal.zeta - env.omega_to_minus_zk_rows)
            + (zeta1m1 * alpha_pow(PERM_ALPHA0 + 2) * (minimal.zeta - F::one())))
            * (F::one() - evals.z.zeta);

    let denominator = (minimal.zeta - env.omega_to_minus_zk_rows) * (minimal.zeta - F::one());
    let ft_eval0 = ft_eval0 + (nominator / denominator);

    let minimal = MinimalForScalar {
        alpha: minimal.alpha,
        beta: minimal.beta,
        gamma: minimal.gamma,
        lookup: minimal.joint_combiner,
    };
    let mut w = Witness::empty();
    let constant_term = scalars::compute(None, &minimal, evals, env, &mut w);

    ft_eval0 - constant_term
}

fn get_feature_flag<F: FieldWitness>(
    feature_flags: &AllFeatureFlags<F>,
    feature: &kimchi::circuits::expr::FeatureFlag,
    w: &mut Witness<F>,
) -> Option<Boolean> {
    use kimchi::circuits::{expr::FeatureFlag::*, lookup::lookups::LookupPattern};

    match feature {
        RangeCheck0 => Some(feature_flags.features.range_check0),
        RangeCheck1 => Some(feature_flags.features.range_check1),
        ForeignFieldAdd => Some(feature_flags.features.foreign_field_add),
        ForeignFieldMul => Some(feature_flags.features.foreign_field_mul),
        Xor => Some(feature_flags.features.xor),
        Rot => Some(feature_flags.features.rot),
        LookupTables => Some(*feature_flags.lookup_tables.get(w)),
        RuntimeLookupTables => Some(feature_flags.features.runtime_tables),
        TableWidth(3) => Some(*feature_flags.table_width_3.get(w)),
        TableWidth(2) => Some(*feature_flags.table_width_at_least_2.get(w)),
        TableWidth(i) if *i <= 1 => Some(*feature_flags.table_width_at_least_1.get(w)),
        TableWidth(_) => None,
        LookupsPerRow(4) => Some(*feature_flags.lookups_per_row_4.get(w)),
        LookupsPerRow(i) if *i <= 3 => Some(*feature_flags.lookups_per_row_3.get(w)),
        LookupsPerRow(_) => None,
        LookupPattern(LookupPattern::Lookup) => Some(feature_flags.features.lookup),
        LookupPattern(LookupPattern::Xor) => Some(*feature_flags.lookup_pattern_xor.get(w)),
        LookupPattern(LookupPattern::RangeCheck) => {
            Some(*feature_flags.lookup_pattern_range_check.get(w))
        }
        LookupPattern(LookupPattern::ForeignFieldMul) => {
            Some(feature_flags.features.foreign_field_mul)
        }
    }
}

mod scalars {
    use std::collections::BTreeMap;

    use kimchi::{
        circuits::{
            berkeley_columns::{BerkeleyChallengeTerm, Column},
            constraints::FeatureFlags,
            expr::{
                CacheId, ConstantExpr, ConstantExprInner, Constants, Expr, ExprError, ExprInner,
                Operations, Variable,
            },
            gate::{CurrOrNext, GateType},
            lookup::lookups::{LookupFeatures, LookupPatterns},
        },
        proof::PointEvaluations,
    };

    use crate::ledger::proofs::transaction::endos;

    use super::*;

    // This method `Variable::evaluate` is private in proof-systems :(
    fn var_evaluate<F: FieldWitness>(
        v: &Variable<Column>,
        evals: &ProofEvaluations<PointEvaluations<F>>,
    ) -> Result<F, ExprError<Column>> {
        let point_evaluations = {
            use kimchi::circuits::lookup::lookups::LookupPattern;
            use Column::*;

            match v.col {
                Witness(i) => Ok(evals.w[i]),
                Z => Ok(evals.z),
                LookupSorted(i) => {
                    evals.lookup_sorted[i].ok_or(ExprError::MissingIndexEvaluation(v.col))
                }
                LookupAggreg => evals
                    .lookup_aggregation
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                LookupTable => evals
                    .lookup_table
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                LookupRuntimeTable => evals
                    .runtime_lookup_table
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::Poseidon) => Ok(evals.poseidon_selector),
                Index(GateType::Generic) => Ok(evals.generic_selector),
                Index(GateType::CompleteAdd) => Ok(evals.complete_add_selector),
                Index(GateType::VarBaseMul) => Ok(evals.mul_selector),
                Index(GateType::EndoMul) => Ok(evals.emul_selector),
                Index(GateType::EndoMulScalar) => Ok(evals.endomul_scalar_selector),
                Index(GateType::RangeCheck0) => evals
                    .range_check0_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::RangeCheck1) => evals
                    .range_check1_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::ForeignFieldAdd) => evals
                    .foreign_field_add_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::ForeignFieldMul) => evals
                    .foreign_field_mul_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::Xor16) => evals
                    .xor_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(GateType::Rot64) => evals
                    .rot_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Permutation(i) => Ok(evals.s[i]),
                Coefficient(i) => Ok(evals.coefficients[i]),
                Column::LookupKindIndex(LookupPattern::Xor) => evals
                    .xor_lookup_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Column::LookupKindIndex(LookupPattern::Lookup) => evals
                    .lookup_gate_lookup_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Column::LookupKindIndex(LookupPattern::RangeCheck) => evals
                    .range_check_lookup_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Column::LookupKindIndex(LookupPattern::ForeignFieldMul) => evals
                    .foreign_field_mul_lookup_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Column::LookupRuntimeSelector => evals
                    .runtime_lookup_table_selector
                    .ok_or(ExprError::MissingIndexEvaluation(v.col)),
                Index(_) => Err(ExprError::MissingIndexEvaluation(v.col)),
            }
        }?;
        match v.row {
            CurrOrNext::Curr => Ok(point_evaluations.zeta),
            CurrOrNext::Next => Ok(point_evaluations.zeta_omega),
        }
    }

    fn pow<F: FieldWitness>(x: F, n: u64, w: &mut Witness<F>) -> F {
        if n == 0 {
            F::one()
        } else if n == 1 {
            x
        } else {
            let y = pow(field::square(x, w), n / 2, w);
            if n.is_multiple_of(2) {
                y
            } else {
                field::mul(x, y, w)
            }
        }
    }

    fn pow_const<F: FieldWitness>(x: F, n: u64) -> F {
        if n == 0 {
            F::one()
        } else if n == 1 {
            x
        } else {
            (0..n - 1).fold(x, |acc, _| x * acc)
        }
    }

    pub struct EvalContext<'a, F: FieldWitness> {
        pub evals: &'a ProofEvaluations<PointEvaluations<F>>,
        pub constants: &'a Constants<F>,
        pub cache: BTreeMap<CacheId, F>,
        pub env: &'a ScalarsEnv<F>,
        pub w: &'a mut Witness<F>,
        pub alpha: F,
        pub beta: F,
        pub gamma: F,
        pub joint_combiner: Option<F>,
    }

    // TODO: Use cvar instead
    fn is_const<F: FieldWitness>(e: &Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>) -> bool {
        match e {
            Expr::Atom(ExprInner::Constant(Operations::Atom(ConstantExprInner::Constant(_)))) => {
                true
            }
            Expr::Pow(x, _) => is_const(x),
            _ => false,
        }
    }

    pub fn eval<F: FieldWitness>(
        e: &Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>,
        ctx: &mut EvalContext<F>,
    ) -> F {
        use Operations;
        match e {
            Operations::Atom(ExprInner::Cell(variable)) => {
                var_evaluate(variable, ctx.evals).unwrap_or_else(|_| F::zero())
            }
            Operations::Atom(ExprInner::VanishesOnZeroKnowledgeAndPreviousRows) => {
                ctx.env.vanishes_on_zero_knowledge_and_previous_rows
            }
            Operations::Atom(ExprInner::UnnormalizedLagrangeBasis(row_offset)) => {
                let unnormalized_lagrange_basis =
                    ctx.env.unnormalized_lagrange_basis.as_ref().unwrap();
                unnormalized_lagrange_basis(*row_offset, ctx.w)
            }
            Operations::Atom(ExprInner::Constant(c)) => sub_eval(c, ctx),
            Operations::Pow(x, p) => {
                let p = *p;
                let v = eval(x, ctx);

                if is_const(x) {
                    pow_const(v, p)
                } else {
                    pow(v, p, ctx.w)
                }
            }
            Operations::Add(x, y) => {
                let y = eval(y, ctx);
                let x = eval(x, ctx);
                x + y
            }
            Operations::Mul(x, y) => {
                let is_x_const = is_const(x);
                let is_y_const = is_const(y);
                let y = eval(y, ctx);
                let x = eval(x, ctx);
                if is_x_const || is_y_const {
                    x * y
                } else {
                    field::mul(x, y, ctx.w)
                }
            }
            Operations::Sub(x, y) => {
                let y = eval(y, ctx);
                let x = eval(x, ctx);
                x - y
            }
            Operations::Double(x) => {
                let v = eval(x, ctx);
                v.double()
            }
            Operations::Square(x) => {
                let is_x_const = is_const(x);
                let x = eval(x, ctx);
                if is_x_const {
                    x * x
                } else {
                    field::mul(x, x, ctx.w)
                }
            }
            Operations::Cache(id, _e) => ctx.cache.get(id).copied().unwrap(),
            Operations::IfFeature(feature, e1, e2) => match ctx.env.feature_flags.as_ref() {
                None => eval(e2, ctx),
                Some(feature_flags) => {
                    let is_feature_enabled = match get_feature_flag(feature_flags, feature, ctx.w) {
                        None => return eval(e2, ctx),
                        Some(enabled) => enabled,
                    };

                    let on_false = eval(e2, ctx);
                    let on_true = eval(e1, ctx);

                    ctx.w.exists_no_check(match is_feature_enabled {
                        Boolean::True => on_true,
                        Boolean::False => on_false,
                    })
                }
            },
        }
    }

    /// This function use to look like following
    /// but `x.value` has changed to accept list of challenges
    ///
    /// let v = x.value(ctx.constants);
    /// if let ConstantExpr::Mul(_, _) = x {
    ///    ctx.w.exists_no_check(v);
    /// };
    /// v
    pub fn sub_eval<F: FieldWitness>(
        e: &Operations<ConstantExprInner<F, BerkeleyChallengeTerm>>,
        ctx: &mut EvalContext<F>,
    ) -> F {
        use Operations;
        match e {
            Operations::Atom(a) => match a {
                ConstantExprInner::Challenge(term) => match term {
                    BerkeleyChallengeTerm::Alpha => ctx.alpha,
                    BerkeleyChallengeTerm::Beta => ctx.beta,
                    BerkeleyChallengeTerm::Gamma => ctx.gamma,
                    BerkeleyChallengeTerm::JointCombiner => ctx.joint_combiner.expect("Unexcepted"),
                },
                ConstantExprInner::Constant(constant_term) => match constant_term {
                    kimchi::circuits::expr::ConstantTerm::EndoCoefficient => {
                        ctx.constants.endo_coefficient
                    }
                    kimchi::circuits::expr::ConstantTerm::Mds { row, col } => {
                        ctx.constants.mds[*row][*col]
                    }
                    kimchi::circuits::expr::ConstantTerm::Literal(literal) => *literal,
                },
            },
            Operations::Pow(x, p) => {
                let p = *p;
                let v = sub_eval(x, ctx);

                pow(v, p, ctx.w)
            }
            Operations::Add(x, y) => {
                let y = sub_eval(y, ctx);
                let x = sub_eval(x, ctx);
                x + y
            }
            Operations::Mul(x, y) => {
                let y = sub_eval(y, ctx);
                let x = sub_eval(x, ctx);
                field::mul(x, y, ctx.w)
            }
            Operations::Sub(x, y) => {
                let x = sub_eval(x, ctx);
                let y = sub_eval(y, ctx);
                x - y
            }
            Operations::Double(x) => {
                let x = sub_eval(x, ctx);
                x.double()
            }
            Operations::Square(x) => {
                let x = sub_eval(x, ctx);
                field::mul(x, x, ctx.w)
            }
            Operations::Cache(id, _e) => ctx.cache.get(id).copied().unwrap(),
            Operations::IfFeature(feature, e1, e2) => match ctx.env.feature_flags.as_ref() {
                None => sub_eval(e2, ctx),
                Some(feature_flags) => {
                    let is_feature_enabled = match get_feature_flag(feature_flags, feature, ctx.w) {
                        None => return sub_eval(e2, ctx),
                        Some(enabled) => enabled,
                    };

                    let on_false = sub_eval(e2, ctx);
                    let on_true = sub_eval(e1, ctx);

                    ctx.w.exists_no_check(match is_feature_enabled {
                        Boolean::True => on_true,
                        Boolean::False => on_false,
                    })
                }
            },
        }
    }

    #[derive(Default)]
    pub struct Cached<F: FieldWitness> {
        /// cache may contain their own caches
        expr: BTreeMap<
            CacheId,
            (
                Box<Cached<F>>,
                Box<Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>>,
            ),
        >,
    }

    #[inline(never)]
    pub fn extract_caches<F: FieldWitness>(
        e: &Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>,
        cache: &mut Cached<F>,
    ) {
        match e {
            Operations::Atom(_) => {}
            Operations::Pow(x, _) => extract_caches(x, cache),
            Operations::Add(x, y) | Operations::Mul(x, y) | Operations::Sub(x, y) => {
                extract_caches(y, cache);
                extract_caches(x, cache);
            }
            Operations::Double(x) => extract_caches(x, cache),
            Operations::Square(x) => extract_caches(x, cache),
            Operations::Cache(id, e) => {
                let mut cached = Cached::default();
                extract_caches(e, &mut cached);
                cache.expr.insert(*id, (Box::new(cached), e.clone()));
            }
            Operations::IfFeature(_, e1, e2) => {
                if false {
                    extract_caches(e1, cache)
                } else {
                    extract_caches(e2, cache)
                }
            }
        }
    }

    fn eval_cache<F: FieldWitness>(cached_exprs: &Cached<F>, ctx: &mut EvalContext<F>) {
        // Each cached expression may contain their own caches
        for (id, (cache, expr)) in &cached_exprs.expr {
            let mut old_cache = std::mem::take(&mut ctx.cache);
            eval_cache::<F>(cache, ctx);
            old_cache.insert(*id, eval::<F>(expr, ctx));
            ctx.cache = old_cache;
        }
    }

    pub struct MinimalForScalar<F> {
        pub alpha: F,
        pub beta: F,
        pub gamma: F,
        pub lookup: Option<F>,
    }

    pub fn compute<F: FieldWitness>(
        gate: Option<GateType>,
        minimal: &MinimalForScalar<F>,
        evals: &ProofEvaluations<PointEvaluations<F>>,
        env: &ScalarsEnv<F>,
        w: &mut Witness<F>,
    ) -> F {
        let (constant_term, index_terms) = &*{
            type TermsMap<F> =
                BTreeMap<Column, Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>>;
            type Const<F> = Expr<ConstantExpr<F, BerkeleyChallengeTerm>, Column>;
            type Terms<F> = Rc<(Const<F>, TermsMap<F>)>;
            cache! {
                Terms::<F>, {
                    // No features for `Fp`:
                    // <https://github.com/MinaProtocol/mina/blob/4af0c229548bc96d76678f11b6842999de5d3b0b/src/lib/crypto/kimchi_bindings/stubs/src/linearization.rs>
                    let is_fp = std::any::TypeId::of::<F>() == std::any::TypeId::of::<Fp>();

                    let features = if is_fp {
                        None
                    } else {
                        Some(FeatureFlags {
                            range_check0: false,
                            range_check1: false,
                            foreign_field_add: false,
                            foreign_field_mul: false,
                            xor: false,
                            rot: false,
                            lookup_features: LookupFeatures {
                                patterns: LookupPatterns {
                                    xor: false,
                                    lookup: false,
                                    range_check: false,
                                    foreign_field_mul: false,
                                },
                                joint_lookup_used: false,
                                uses_runtime_tables: false,
                            },
                        })
                    };

                    let evaluated_cols =
                        kimchi::linearization::linearization_columns::<F>(features.as_ref());
                    let (linearization, _powers_of_alpha) =
                        kimchi::linearization::constraints_expr::<F>(features.as_ref(), true);

                    let kimchi::circuits::expr::Linearization {
                        constant_term,
                        index_terms,
                    } = linearization.linearize(evaluated_cols).unwrap();

                    let index_terms = index_terms.into_iter().collect::<TermsMap<F>>();
                    Rc::new((constant_term, index_terms))
                }
            }
        };

        // Moved internally
        let constants = kimchi::circuits::expr::Constants::<F> {
            endo_coefficient: {
                let (base, _) = endos::<F>();
                base
            },
            mds: &<F::OtherCurve>::sponge_params().mds,
            zk_rows: 3,
        };

        let mut ctx = EvalContext {
            evals,
            constants: &constants,
            cache: BTreeMap::new(),
            env,
            w,
            alpha: minimal.alpha,
            beta: minimal.beta,
            gamma: minimal.gamma,
            joint_combiner: minimal.lookup,
        };

        let term = match gate {
            Some(gate) => index_terms.get(&Column::Index(gate)).unwrap(),
            None => constant_term,
        };

        // We evaluate the cached expressions first
        let mut cached_exprs = Cached::default();
        extract_caches(term, &mut cached_exprs);
        eval_cache(&cached_exprs, &mut ctx);

        // Eval the rest
        eval(term, &mut ctx)
    }
}

// TODO: De-duplicate with `ft_eval0`
