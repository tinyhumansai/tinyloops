//! The shipped threshold sets, each naming the bet it makes.
//!
//! A threshold is a methodological commitment wearing a number's clothes. A
//! `stuck` of 2 says "two consecutive unproductive passes is where sequential
//! revision stops beating parallel sampling", which is arguable, revisable, and
//! domain-dependent. A `stuck` of 2 with nothing written beside it says the
//! same thing and gives nobody a way to argue with it.
//!
//! So every preset here carries its bet in rustdoc, and [`Preset::ALL`] is the
//! one list the exhaustive jq-versus-Rust parity sweep iterates. A preset added
//! without being swept is a preset whose generated ladder nobody proved agrees
//! with [`route`](crate::route).

use crate::policy::Thresholds;

/// A shipped threshold set.
///
/// The variants differ in what they bet about *persistence versus variation*,
/// which is the one axis where the evidence is genuinely split and where the
/// right answer depends on how accurate a domain's feedback is. Sequential
/// self-revision beats drawing several independent attempts only while feedback
/// accuracy is high; below that, sampling and selecting wins. Every preset here
/// is an estimate of which side of that crossing a domain sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Preset {
    /// The shipped default: [`Thresholds::default`].
    ///
    /// The bet: feedback is good enough to be worth acting on twice and not a
    /// third time. It is the set every other one here is a deliberate deviation
    /// from, and the set a domain should start on before it has measured
    /// anything.
    Balanced,
    /// Revise for longer before drawing a fresh approach.
    ///
    /// The bet: **persistence is cheaper than variation.** It belongs where a
    /// judge's verdict is close to ground truth — a compiler, a test suite, a
    /// proof checker — because sequential revision on accurate feedback
    /// converges, and restarting throws away a partial answer that was nearly
    /// right. It costs more attempts per goal, and its failure mode is grinding
    /// on a framing that was wrong from the first pass.
    Persistent,
    /// Draw a fresh approach at the first sign of a stall.
    ///
    /// The bet: **variation is cheaper than persistence.** It belongs where the
    /// evaluator is itself a model and its verdicts are noisy, because revision
    /// conditioned on an unreliable signal compounds the noise while
    /// independent samples do not. It spends its attempts wider and shallower,
    /// and its failure mode is abandoning an approach one pass before it would
    /// have worked.
    Exploratory,
    /// Report a thinly-supported answer rather than banking it.
    ///
    /// The bet: **an unsupported answer is worse than no answer.** It drops
    /// `unverified` to one, so a single pass reaching a conclusion by exactly
    /// one route hands the run over as a report instead of claiming it. It
    /// belongs where a wrong answer is expensive to discover downstream, and
    /// its cost is runs that stop with a usable answer they declined to call
    /// finished.
    Cautious,
}

impl Preset {
    /// Every shipped preset, in a fixed order.
    ///
    /// The parity sweep in `src/policy/test.rs` iterates exactly this, so a
    /// preset cannot be added without its generated ladder being proved against
    /// [`route`](crate::route) over the whole bounded counter space.
    pub const ALL: [Self; 4] = [
        Self::Balanced,
        Self::Persistent,
        Self::Exploratory,
        Self::Cautious,
    ];

    /// The preset's name, for a log line or a configuration file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Persistent => "persistent",
            Self::Exploratory => "exploratory",
            Self::Cautious => "cautious",
        }
    }

    /// Reads a preset back, or `None`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.as_str() == name)
    }

    /// The thresholds this preset stands for.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Preset;
    /// assert!(Preset::Exploratory.thresholds().stuck < Preset::Persistent.thresholds().stuck);
    /// assert_eq!(Preset::Cautious.thresholds().unverified, 1);
    /// ```
    #[must_use]
    pub fn thresholds(self) -> Thresholds {
        match self {
            Self::Balanced => Thresholds::default(),
            Self::Persistent => Thresholds {
                stuck: 4,
                max_attempts: 12,
                ..Thresholds::default()
            },
            Self::Exploratory => Thresholds {
                stuck: 1,
                computational: 1,
                ..Thresholds::default()
            },
            Self::Cautious => Thresholds {
                unverified: 1,
                ..Thresholds::default()
            },
        }
    }
}

impl std::fmt::Display for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
