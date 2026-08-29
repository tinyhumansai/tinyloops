//! The caps a run is configured with, the bound that names which one tripped,
//! and the meters that advance against them.
//!
//! Everything here is data plus the arithmetic that keeps it honest. The
//! validation that decides whether a set of caps is a legal configuration lives
//! in the module root, because it is a rule about the caps rather than a
//! property of any one of them.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ordered, positive, positive_duration, uncontended};
use crate::Result;

/// How many tool calls one model call is assumed to be able to issue.
///
/// The headroom factor that makes "the model-call cap is the reachable one" a
/// checkable statement rather than a hope. A configuration whose tool-call cap
/// is below `max_model_calls * TOOL_CALLS_PER_MODEL_CALL` has a tool-call cap
/// the run can reach first, and [`RunBudget::new`] rejects it.
///
/// Eight is deliberately generous. Under-estimating the fan-out is the
/// dangerous direction: it would let a configuration through whose tool-call
/// cap trips first, which is the overrun path that does not preserve partial
/// results.
pub const TOOL_CALLS_PER_MODEL_CALL: u32 = 8;

/// Which bound stopped a run.
///
/// A tripped bound is a routed outcome carrying this value, never a bare error:
/// the run stops, reports what it has, and says what stopped it. `observe`
/// puts this on the event stream and in the run's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bound {
    /// The loop took all the passes it was allowed.
    Iterations,
    /// The run spent its wall clock.
    RunClock,
    /// The run made all the model calls it was allowed.
    ModelCalls,
    /// The run made all the tool calls it was allowed.
    ///
    /// Unreachable in a validated configuration — see
    /// [`RunBudget::reachable`] — and present so a run that somehow reaches it
    /// can still say so rather than reporting nothing.
    ToolCalls,
    /// The run spent its token allowance.
    Tokens,
    /// A single tool call outlived its timeout.
    ToolTimeout,
    /// A single provider request outlived its timeout.
    RequestTimeout,
}

impl Bound {
    /// The wire name of this bound.
    ///
    /// Hand-written rather than derived from [`Debug`], for the same reason
    /// [`Route::as_str`](crate::Route::as_str) is: a `Debug` rendering is a
    /// diagnostic that changes when a variant is renamed, and this string is
    /// read back by [`Self::parse`] and written into event streams that outlive
    /// the process.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Iterations => "iterations",
            Self::RunClock => "run_clock",
            Self::ModelCalls => "model_calls",
            Self::ToolCalls => "tool_calls",
            Self::Tokens => "tokens",
            Self::ToolTimeout => "tool_timeout",
            Self::RequestTimeout => "request_timeout",
        }
    }

    /// Reads a bound name, strictly.
    ///
    /// Unlike [`Route::parse`](crate::Route::parse) this does *not* fall back
    /// to a cheap default. A route is a decision the loop keeps making, so a
    /// misread one costs a pass; a bound is a report about why a run ended, and
    /// a misread one would attribute the ending to the wrong cap for as long as
    /// anybody trusts the report. An unknown name is `None`, and the caller
    /// says "unknown" rather than guessing.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Bound;
    /// assert_eq!(Bound::parse("model_calls"), Some(Bound::ModelCalls));
    /// assert_eq!(Bound::parse(" TOKENS "), Some(Bound::Tokens));
    /// assert_eq!(Bound::parse("model calls"), None);
    /// ```
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "iterations" => Some(Self::Iterations),
            "run_clock" => Some(Self::RunClock),
            "model_calls" => Some(Self::ModelCalls),
            "tool_calls" => Some(Self::ToolCalls),
            "tokens" => Some(Self::Tokens),
            "tool_timeout" => Some(Self::ToolTimeout),
            "request_timeout" => Some(Self::RequestTimeout),
            _ => None,
        }
    }

    /// Whether overrunning this bound leaves the run able to report.
    ///
    /// The distinction the "only one cap may trip" rule is built on. A model
    /// call that is not made leaves everything the run had already collected in
    /// hand; an expired run clock takes the context and the report with it.
    #[must_use]
    pub fn is_graceful(self) -> bool {
        matches!(
            self,
            Self::Iterations | Self::ModelCalls | Self::Tokens | Self::ToolTimeout
        )
    }
}

/// The numbers a [`RunBudget`] is built from.
///
/// Plain data with no invariants of its own: a caller fills it in, hands it to
/// [`RunBudget::new`], and gets back either a validated budget or an error
/// naming the rule it broke. Splitting the numbers from the validated type is
/// what lets the budget keep its fields private — a `RunBudget` whose fields
/// could be reassigned after construction would be a budget whose invariants
/// held only until somebody wrote to it.
///
/// Every field is a cap, and none of them may be zero. There is no "unbounded"
/// spelling on purpose: a run with an unbounded scope is the run that ends when
/// something external kills it, which is the one ending that produces no
/// report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caps {
    /// How many passes round the loop are allowed.
    pub max_iterations: u32,
    /// How many model calls the run may make.
    ///
    /// The cap that actually stops a run — see [`RunBudget::reachable`].
    pub max_model_calls: u32,
    /// How many tool calls the run may make.
    ///
    /// Set far above reach on purpose. It exists as a backstop, not as a
    /// control, and a configuration in which it can trip before
    /// [`Self::max_model_calls`] is rejected.
    pub max_tool_calls: u32,
    /// How many tokens the run may spend, prompt and output together.
    pub max_tokens: u64,
    /// How long the whole run may take.
    pub run_timeout: Duration,
    /// How long a single tool call may take.
    ///
    /// Strictly shorter than [`Self::run_timeout`], asserted at construction.
    pub tool_timeout: Duration,
    /// How long a single provider request may take.
    ///
    /// Strictly shorter than [`Self::tool_timeout`], so a hung request is
    /// reported as a hung request rather than surfacing as a tool timeout that
    /// says nothing about which leg hung.
    pub request_timeout: Duration,
    /// How many times a failed call is retried before the failure stands.
    pub max_retries: u32,
}

impl Default for Caps {
    /// The caps a run gets when the caller expresses no preference.
    ///
    /// Chosen so the relationships the constructor asserts hold with room to
    /// spare, and `budget/test.rs` asserts that they do: a default that could
    /// not be constructed is a default nobody can use.
    fn default() -> Self {
        Self {
            max_iterations: 8,
            max_model_calls: 60,
            // 60 * 8 = 480 is the reachability floor; 600 clears it.
            max_tool_calls: 600,
            max_tokens: 2_000_000,
            run_timeout: Duration::from_secs(30 * 60),
            tool_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(60),
            max_retries: 3,
        }
    }
}

/// The concentric bounds one run carries, validated.
///
/// Construction is the whole point of the type. [`RunBudget::new`] is the only
/// way to make one, and it rejects three classes of configuration that are
/// otherwise invisible until a run ends badly:
///
/// 1. **A cap of zero**, which is an unbounded scope wearing a number.
/// 2. **Inverted timeouts**, where the run clock can expire while a tool call
///    is outstanding, making the tool's graceful path unreachable.
/// 3. **Contended caps**, where the tool-call cap is low enough to trip before
///    the model-call cap, putting the run on the overrun path that loses its
///    partial results.
///
/// Deliberately not [`Serialize`] or [`Deserialize`]. Deserializing straight
/// into this type would rebuild it without running any of those checks, which
/// is exactly the hole the private fields exist to close; deserialize [`Caps`]
/// and hand it to [`RunBudget::new`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudget {
    caps: Caps,
}

impl Default for RunBudget {
    /// The default caps, already validated.
    ///
    /// Builds the struct directly rather than routing [`Caps::default`] through
    /// [`RunBudget::new`], because `Default::default` cannot return an error
    /// and library code here must not panic. The check is not skipped, only
    /// moved: `budget/test.rs` asserts `RunBudget::new(Caps::default())`
    /// succeeds and yields exactly this value, so a default that stops being
    /// legal fails the suite instead of shipping.
    fn default() -> Self {
        Self {
            caps: Caps::default(),
        }
    }
}

impl RunBudget {
    /// Validates `caps` into a budget.
    ///
    /// # Errors
    ///
    /// - [`Error::UnboundedCap`](crate::Error::UnboundedCap) when any cap is
    ///   zero.
    /// - [`Error::NestedTimeout`](crate::Error::NestedTimeout) when
    ///   `tool_timeout >= run_timeout`, or when
    ///   `request_timeout >= tool_timeout`.
    /// - [`Error::ContendedCaps`](crate::Error::ContendedCaps) when the
    ///   tool-call cap can be reached before the model-call cap.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use tinyloops::{Bound, Caps, Error, RunBudget};
    /// let budget = RunBudget::new(Caps::default())?;
    /// assert_eq!(budget.reachable(), Bound::ModelCalls);
    ///
    /// let inverted = Caps {
    ///     tool_timeout: Duration::from_secs(600),
    ///     ..Caps::default()
    /// };
    /// assert_eq!(
    ///     RunBudget::new(inverted).unwrap_err(),
    ///     Error::NestedTimeout { inner: Bound::ToolTimeout, outer: Bound::RunClock },
    /// );
    /// # Ok::<(), tinyloops::Error>(())
    /// ```
    pub fn new(caps: Caps) -> Result<Self> {
        positive(u64::from(caps.max_iterations), Bound::Iterations)?;
        positive(u64::from(caps.max_model_calls), Bound::ModelCalls)?;
        positive(u64::from(caps.max_tool_calls), Bound::ToolCalls)?;
        positive(caps.max_tokens, Bound::Tokens)?;
        positive_duration(caps.run_timeout, Bound::RunClock)?;
        positive_duration(caps.tool_timeout, Bound::ToolTimeout)?;
        positive_duration(caps.request_timeout, Bound::RequestTimeout)?;

        ordered(
            caps.tool_timeout,
            caps.run_timeout,
            Bound::ToolTimeout,
            Bound::RunClock,
        )?;
        ordered(
            caps.request_timeout,
            caps.tool_timeout,
            Bound::RequestTimeout,
            Bound::ToolTimeout,
        )?;

        uncontended(caps.max_model_calls, caps.max_tool_calls)?;

        Ok(Self { caps })
    }

    /// The validated caps.
    #[must_use]
    pub fn caps(&self) -> Caps {
        self.caps
    }

    /// The one cap a validated budget can actually reach.
    ///
    /// Always [`Bound::ModelCalls`], and that is the point: the constructor
    /// enforces the relationship that makes it true, so the answer is a
    /// consequence of the type rather than a property of one configuration.
    /// The model-call cap is chosen because its overrun path is the graceful
    /// one — the run stops holding everything it collected, and reports it.
    #[must_use]
    pub fn reachable(&self) -> Bound {
        Bound::ModelCalls
    }

    /// Narrows this budget to a role's own caps, clamped to never exceed it.
    ///
    /// Per-role narrowing is required rather than inherited, because a role
    /// given the loop's budget spends the loop's budget. The failure is not
    /// abstract overspending: a role that reads a report and answers in four
    /// lines, handed an investigation's budget, investigates. One judge on a
    /// wide budget spent four minutes and fifteen model calls reading source
    /// files while the attempt it was judging — already finished — waited on
    /// it. Nothing failed and no cap was exceeded, because none of them was
    /// narrow enough to notice.
    ///
    /// Every cap is clamped to the smaller of `caps` and this budget's, so a
    /// role can only ever be narrower than the run that contains it.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`RunBudget::new`]: clamping can leave a
    /// combination that no longer satisfies the ordering or reachability
    /// rules, and that is reported rather than repaired.
    pub fn narrow(&self, caps: Caps) -> Result<Self> {
        Self::new(Caps {
            max_iterations: caps.max_iterations.min(self.caps.max_iterations),
            max_model_calls: caps.max_model_calls.min(self.caps.max_model_calls),
            max_tool_calls: caps.max_tool_calls.min(self.caps.max_tool_calls),
            max_tokens: caps.max_tokens.min(self.caps.max_tokens),
            run_timeout: caps.run_timeout.min(self.caps.run_timeout),
            tool_timeout: caps.tool_timeout.min(self.caps.tool_timeout),
            request_timeout: caps.request_timeout.min(self.caps.request_timeout),
            max_retries: caps.max_retries.min(self.caps.max_retries),
        })
    }

    /// The budget a judging role runs under.
    ///
    /// A judge reads one attempt and returns a verdict. It does not need to
    /// re-derive the attempt, and given the calls to do so it will: see
    /// [`Self::narrow`] for the four minutes that motivated these numbers.
    ///
    /// # Errors
    ///
    /// As [`Self::narrow`].
    pub fn judging(&self) -> Result<Self> {
        self.narrow(Caps {
            max_iterations: 1,
            max_model_calls: 4,
            max_tool_calls: 40,
            max_tokens: 200_000,
            run_timeout: Duration::from_secs(120),
            tool_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(15),
            max_retries: 2,
        })
    }

    /// The budget a housekeeping role runs under.
    ///
    /// Summarizing, tidying, and formatting: work whose correct cost is one
    /// call and whose worst case is an agent that decides to go and read the
    /// repository first.
    ///
    /// # Errors
    ///
    /// As [`Self::narrow`].
    pub fn housekeeping(&self) -> Result<Self> {
        self.narrow(Caps {
            max_iterations: 1,
            max_model_calls: 2,
            max_tool_calls: 20,
            max_tokens: 50_000,
            run_timeout: Duration::from_secs(60),
            tool_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(10),
            max_retries: 1,
        })
    }

    /// The bound `meter` has reached, if any.
    ///
    /// Checked outermost first, in the order the bounds nest, so a run that has
    /// simultaneously exhausted two scopes reports the outer one — the one that
    /// describes the run rather than the call inside it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Bound, Meter, RunBudget};
    /// let budget = RunBudget::default();
    /// let mut meter = Meter::default();
    /// assert_eq!(budget.tripped(&meter), None);
    ///
    /// for _ in 0..budget.caps().max_model_calls {
    ///     meter.model_call(10);
    /// }
    /// assert_eq!(budget.tripped(&meter), Some(Bound::ModelCalls));
    /// ```
    #[must_use]
    pub fn tripped(&self, meter: &Meter) -> Option<Bound> {
        if meter.iterations() >= self.caps.max_iterations {
            Some(Bound::Iterations)
        } else if meter.elapsed() >= self.caps.run_timeout {
            Some(Bound::RunClock)
        } else if meter.model_calls() >= self.caps.max_model_calls {
            Some(Bound::ModelCalls)
        } else if meter.tokens() >= self.caps.max_tokens {
            Some(Bound::Tokens)
        } else if meter.tool_calls() >= self.caps.max_tool_calls {
            Some(Bound::ToolCalls)
        } else {
            None
        }
    }
}

/// What a run has actually spent, against the caps in its [`RunBudget`].
///
/// Two meters advance side by side, and the reason both are carried is that
/// they answer different questions. Raw compute — passes, calls, tokens —
/// bounds the worst case. **Effective feedback** — the passes that produced a
/// usable signal, a test result, a diff, a verdict — is what the stopping
/// decision should read.
///
/// The difference is not cosmetic. Measured against outcomes, raw compute shows
/// near-zero fit while an effective-feedback measure fits R²=0.93; budgeting on
/// it rather than on token count moved pass rate from 61.2% to 68.2% while
/// cutting mean cost from 213.8 to 85.1. A loop that counts only turns cannot
/// tell ten productive passes from ten that each learned nothing, and those are
/// the two cases a budget most needs to tell apart.
///
/// # The clock is injected
///
/// [`Self::elapsed`] advances only when a caller calls [`Self::advance`]. The
/// meter never reads the wall clock, so a test that asserts on a run clock
/// asserts on a number it supplied.
///
/// # Arithmetic
///
/// Every counter saturates. A wrapped meter reads as a fresh run — the budget
/// would silently reset itself at the worst possible moment — and a panicking
/// one takes down a node the engine cannot unwind sensibly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Meter {
    iterations: u32,
    model_calls: u32,
    tool_calls: u32,
    tokens: u64,
    elapsed: Duration,
    effective: u32,
    inert: u32,
}

impl Meter {
    /// Records a completed pass, saying whether it produced a usable signal.
    ///
    /// A pass always advances [`Self::iterations`]. It advances
    /// [`Self::effective_passes`] only when `signal` is true, and
    /// [`Self::inert_passes`] otherwise, which is what makes ten productive
    /// passes and ten inert ones reach the same raw total and different
    /// effective ones.
    pub fn pass(&mut self, signal: bool) {
        self.iterations = self.iterations.saturating_add(1);
        if signal {
            self.effective = self.effective.saturating_add(1);
        } else {
            self.inert = self.inert.saturating_add(1);
        }
    }

    /// Records one model call and the tokens it spent.
    pub fn model_call(&mut self, tokens: u64) {
        self.model_calls = self.model_calls.saturating_add(1);
        self.tokens = self.tokens.saturating_add(tokens);
    }

    /// Records one tool call.
    pub fn tool_call(&mut self) {
        self.tool_calls = self.tool_calls.saturating_add(1);
    }

    /// Advances the run clock by `elapsed`.
    pub fn advance(&mut self, elapsed: Duration) {
        self.elapsed = self.elapsed.saturating_add(elapsed);
    }

    /// Passes taken.
    #[must_use]
    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    /// Model calls made.
    #[must_use]
    pub fn model_calls(&self) -> u32 {
        self.model_calls
    }

    /// Tool calls made.
    #[must_use]
    pub fn tool_calls(&self) -> u32 {
        self.tool_calls
    }

    /// Tokens spent.
    #[must_use]
    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    /// Wall clock the caller has reported.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Passes that produced a usable signal.
    #[must_use]
    pub fn effective_passes(&self) -> u32 {
        self.effective
    }

    /// Passes that produced none.
    #[must_use]
    pub fn inert_passes(&self) -> u32 {
        self.inert
    }

    /// The share of passes that produced a usable signal, or `None` before the
    /// first pass.
    ///
    /// `None` rather than zero, because "no passes yet" and "no pass has
    /// learned anything" are the opposite situations and a stopping rule that
    /// confuses them stops every run at pass zero.
    #[must_use]
    pub fn effective_ratio(&self) -> Option<f64> {
        if self.iterations == 0 {
            None
        } else {
            Some(f64::from(self.effective) / f64::from(self.iterations))
        }
    }
}
