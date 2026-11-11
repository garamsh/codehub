//! The one derivation mechanism: every derived type is a step from its values for a changeset's
//! parents and the changeset, flagged eager or deferred (ADR 0011).

use std::collections::BTreeMap;
use std::fmt;

use super::address::Address;
use super::changeset::Changeset;

/// When a derived type is computed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flag {
    /// At landing.
    Eager,
    /// On the first query that needs it.
    Deferred,
}

/// A derived type: when it is computed, and its step from the parents' values.
pub trait Derivation {
    /// When this type is computed.
    const FLAG: Flag;
    /// This type's value for one changeset.
    type Value;
    /// The content-addressed objects a step writes.
    type Object;
    /// Why a step fails.
    type Error;

    /// Derives `changeset`'s value from `parents`, one value per entry of `changeset.parents`
    /// and in that order. [`derive`] checks that correspondence before it calls this.
    ///
    /// # Errors
    ///
    /// The type's own [`Derivation::Error`].
    fn step(
        &self,
        changeset: &Changeset,
        parents: &[Self::Value],
    ) -> Result<Step<Self::Value, Self::Object>, Self::Error>;
}

/// What a step produces.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step<V, O> {
    /// The changeset's value.
    pub value: V,
    /// Each object the step wrote, by address.
    pub objects: BTreeMap<Address, O>,
}

/// Why a derivation fails.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DerivationError<E> {
    /// The number of parent values is not the number of the changeset's parents.
    ParentCount {
        /// The changeset's parents.
        expected: usize,
        /// The parent values given.
        given: usize,
    },
    /// The type's step failed.
    Step(E),
}

impl<E: fmt::Display> fmt::Display for DerivationError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParentCount { expected, given } => {
                write!(f, "{given} parent values for {expected} parents")
            }
            Self::Step(error) => write!(f, "{error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for DerivationError<E> {}

/// What a step of `D` produces.
pub type StepOf<D> = Step<<D as Derivation>::Value, <D as Derivation>::Object>;

/// Derives `changeset`'s value of type `D` from its parents' values, given in parent order.
///
/// # Errors
///
/// [`DerivationError::ParentCount`] where `parents` does not hold one value per parent, and
/// [`DerivationError::Step`] where the step fails.
pub fn derive<D: Derivation>(
    derivation: &D,
    changeset: &Changeset,
    parents: &[D::Value],
) -> Result<StepOf<D>, DerivationError<D::Error>> {
    if parents.len() != changeset.parents.len() {
        return Err(DerivationError::ParentCount {
            expected: changeset.parents.len(),
            given: parents.len(),
        });
    }
    derivation
        .step(changeset, parents)
        .map_err(DerivationError::Step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file_version::FileVersions;
    use crate::domain::tree::Trees;

    #[test]
    fn flags_trees_eager_and_file_versions_deferred() {
        assert_eq!(Trees::FLAG, Flag::Eager);
        assert_eq!(FileVersions::FLAG, Flag::Deferred);
    }

    #[test]
    fn refuses_parent_values_that_do_not_match_the_parents() {
        let changeset = Changeset {
            parents: vec![Address::of(b"first")],
            author: "ada".to_owned(),
            timestamp: 0,
            message: String::new(),
            changes: BTreeMap::new(),
        };
        // Control: one value for the one parent.
        assert!(derive(&FileVersions, &changeset, &[BTreeMap::new()]).is_ok());

        assert_eq!(
            derive(&FileVersions, &changeset, &[]),
            Err(DerivationError::ParentCount {
                expected: 1,
                given: 0
            })
        );
        assert_eq!(
            derive(
                &FileVersions,
                &changeset,
                &[BTreeMap::new(), BTreeMap::new()]
            ),
            Err(DerivationError::ParentCount {
                expected: 1,
                given: 2
            })
        );
    }
}
