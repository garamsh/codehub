//! How many times a use case tries again where its landing lost the compare-and-swap on `main`.

use std::num::NonZeroU32;

/// The most times a landing made by the server itself is tried where `main` keeps moving under
/// it; at least one. A parameter of the service that lands, not a constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LandingAttempts(NonZeroU32);

impl LandingAttempts {
    /// The number of attempts when none is chosen.
    pub const DEFAULT: NonZeroU32 = match NonZeroU32::new(32) {
        Some(attempts) => attempts,
        None => NonZeroU32::MIN,
    };

    /// `attempts` tries.
    #[must_use]
    pub const fn new(attempts: NonZeroU32) -> Self {
        Self(attempts)
    }

    /// The number of tries.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl Default for LandingAttempts {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}
