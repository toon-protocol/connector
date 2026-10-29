//! The tiers: the small published set of budgets an allowance may be opened for.
//!
//! A payer chooses from the set so that its budget doesn't single it out. The set is an
//! operator's to publish, so a [`Tiers`] is a validated list and not a constant -- but every
//! tier in it must be one of zk_toon's `policy::TIERS` (ADR 0080 decision 3, as amended), so
//! every zk edge stays on the crowd's published values.

/// A budget the zk edge publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tier(u64);

impl Tier {
    /// The budget, in the vault asset's smallest unit.
    #[must_use]
    pub const fn budget(self) -> u64 {
        self.0
    }
}

/// Why a set of tiers is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiersError {
    /// No tier at all: nothing could be opened.
    Empty,
    /// A budget of zero pays nothing.
    Zero,
    /// A budget appears twice.
    Duplicate,
    /// A budget that is not one of zk_toon's published tiers.
    NotPublished(u64),
}

impl std::fmt::Display for TiersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "no tier is published",
            Self::Zero => "a tier of zero pays nothing",
            Self::Duplicate => "a tier is published twice",
            Self::NotPublished(tier) => {
                return write!(
                    f,
                    "{tier} is not one of zk_toon's tiers {:?}",
                    zk_note::policy::TIERS
                )
            }
        })
    }
}

impl std::error::Error for TiersError {}

/// A published set of tiers: non-empty, no zero budget, no repeat, each one of zk_toon's,
/// sorted ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tiers(Vec<Tier>);

impl Tiers {
    /// Validate a set of budgets.
    ///
    /// # Errors
    /// [`TiersError`] if the set is empty, holds a zero, repeats a budget, or names a budget
    /// zk_toon does not publish.
    pub fn new(budgets: impl IntoIterator<Item = u64>) -> Result<Self, TiersError> {
        let mut v: Vec<u64> = budgets.into_iter().collect();
        if v.is_empty() {
            return Err(TiersError::Empty);
        }
        if v.contains(&0) {
            return Err(TiersError::Zero);
        }
        if let Some(tier) = v.iter().find(|b| !zk_note::policy::is_tier(**b)) {
            return Err(TiersError::NotPublished(*tier));
        }
        v.sort_unstable();
        if v.windows(2).any(|w| w[0] == w[1]) {
            return Err(TiersError::Duplicate);
        }
        Ok(Self(v.into_iter().map(Tier).collect()))
    }

    /// The tier for `budget`, if it is published.
    #[must_use]
    pub fn get(&self, budget: u64) -> Option<Tier> {
        self.0.iter().copied().find(|t| t.0 == budget)
    }

    /// The tiers, ascending.
    #[must_use]
    pub fn as_slice(&self) -> &[Tier] {
        &self.0
    }
}
