//! The loop's event vocabulary and the views derived from it.
//!
//! Everything here is data: the [`Event`] a loop transition emits, the
//! per-call records [`ModelCall`] and [`ToolCall`], the cumulative [`Spend`]
//! and [`Accounting`] read off them, and the [`Report`] a finished run returns.
//! The recorder that receives events and the sinks that render them live in the
//! module root.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::budget::Bound;
use crate::policy::{Judgement, Outcome, Route};
use crate::state::Delta;

/// What a run captures of the text flowing through it.
///
/// **Both fields default to `false`, and that default is the point.**
/// Observability that defaults to recording prompts is a secret leak with a
/// dashboard attached: the deployment that wanted payload capture asks for it,
/// and the deployment that never thought about it does not get it by accident.
///
/// Capture is applied by [`Recorder::record`](super::Recorder::record) before
/// an event reaches the journal or any sink, so a payload that was not opted
/// into does not exist downstream — it is not merely unrendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    /// Whether prompts and completions are kept on [`ModelCall`].
    pub model_io: bool,
    /// Whether arguments and output are kept on [`ToolCall`].
    pub tool_io: bool,
}

impl Capture {
    /// Captures both model and tool payloads.
    ///
    /// Pair this with a [`RedactingSink`](super::RedactingSink): capture is the
    /// decision to record text, and redaction is what keeps the recorded text
    /// from carrying credentials into a log.
    #[must_use]
    pub fn all() -> Self {
        Self {
            model_io: true,
            tool_io: true,
        }
    }
}

/// One model call, as the provider's response described it.
///
/// **Every number here is read off the response body, never from a local price
/// table.** With a fallback ladder the route genuinely varies per call: the
/// same logical request may be served by a different provider or a cheaper
/// model tier than the one configured, and a local table prices the request
/// that was intended rather than the one that happened. The crate therefore has
/// no price table at all, and a provider that reports no cost yields
/// [`cost: None`](Self::cost) rather than an invented figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCall {
    /// The provider that answered, as it identified itself.
    pub provider: String,
    /// The model that answered, which is not necessarily the one configured.
    pub model: String,
    /// The role whose budget this call was made under.
    pub role: String,
    /// Prompt tokens the response reported.
    pub prompt_tokens: u64,
    /// Prompt tokens the response reported as served from cache.
    pub cached_tokens: u64,
    /// Output tokens the response reported.
    pub output_tokens: u64,
    /// The share of the prompt served from cache, as the provider reported it.
    ///
    /// A first-class field rather than something derived from the token counts
    /// later, because it is the signal that arrives first. The hardest-to-find
    /// harness regression on public record showed up as continuous cache misses
    /// burning through rate limits while every other signal looked normal; a
    /// hit rate emitted per call turns that into a visible step change at the
    /// moment the regression lands, instead of a number nobody computed.
    ///
    /// Use [`Self::hit_rate_from_tokens`] when the provider reports token
    /// counts but no rate.
    pub cache_hit_rate: f64,
    /// What the provider said the call cost, or `None` when it said nothing.
    pub cost: Option<f64>,
    /// The prompt, present only under [`Capture::model_io`].
    pub prompt: Option<String>,
    /// The completion, present only under [`Capture::model_io`].
    pub completion: Option<String>,
}

impl ModelCall {
    /// An empty call record for `provider`, `model`, and `role`.
    ///
    /// The counts and the payloads are filled in from the response by the
    /// caller; there are too many of them for a constructor argument list that
    /// anybody could read at a call site.
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            role: role.into(),
            prompt_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cache_hit_rate: 0.0,
            cost: None,
            prompt: None,
            completion: None,
        }
    }

    /// The cached share of `prompt` tokens, for a provider that reports counts
    /// but no rate.
    ///
    /// Zero for an empty prompt, which is the honest answer: nothing was asked
    /// for, so nothing was missed.
    #[must_use]
    pub fn hit_rate_from_tokens(prompt: u64, cached: u64) -> f64 {
        if prompt == 0 {
            0.0
        } else {
            ratio(cached.min(prompt), prompt)
        }
    }

    /// Drops the captured payloads.
    #[must_use]
    pub(super) fn without_payloads(mut self) -> Self {
        self.prompt = None;
        self.completion = None;
        self
    }
}

/// One tool call, as the harness reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The tool that ran.
    pub tool: String,
    /// The role whose budget this call was made under.
    pub role: String,
    /// How long it took.
    pub duration: Duration,
    /// Whether it hit its timeout.
    ///
    /// A timed-out tool call still returns the output it captured, so this is a
    /// status on a completed record rather than the absence of one.
    pub timed_out: bool,
    /// The arguments, present only under [`Capture::tool_io`].
    pub arguments: Option<String>,
    /// The output, present only under [`Capture::tool_io`].
    pub output: Option<String>,
}

impl ToolCall {
    /// An empty call record for `tool` and `role`.
    #[must_use]
    pub fn new(tool: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            role: role.into(),
            duration: Duration::ZERO,
            timed_out: false,
            arguments: None,
            output: None,
        }
    }

    /// Drops the captured payloads.
    #[must_use]
    pub(super) fn without_payloads(mut self) -> Self {
        self.arguments = None;
        self.output = None;
        self
    }
}

/// The movement one merge folded into the loop's accumulator.
///
/// A wire-shaped mirror of [`Delta`], which is deliberately not serializable —
/// it is an in-process fold input, not a payload. Reporting the merge means
/// reporting numbers, so the numbers are copied into a type that can be written
/// to a journal without giving `Delta` a wire form it does not otherwise need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Movement {
    /// Movement in `passes`.
    pub passes: i64,
    /// Movement in `attempts`.
    pub attempts: i64,
    /// Movement in `unproductive`.
    pub unproductive: i64,
    /// Movement in `blocked`.
    pub blocked: i64,
    /// Movement in `computational`.
    pub computational: i64,
    /// Movement in `unverified`.
    pub unverified: i64,
    /// Movement in `restarts`.
    pub restarts: i64,
    /// Movement in `established`.
    pub established: i64,
    /// Movement in `banked`.
    pub banked: i64,
    /// The vote cast on `solved`.
    pub solved: Option<bool>,
    /// The vote cast on `expired`.
    pub expired: Option<bool>,
}

impl From<&Delta> for Movement {
    fn from(delta: &Delta) -> Self {
        Self {
            passes: delta.passes,
            attempts: delta.attempts,
            unproductive: delta.unproductive,
            blocked: delta.blocked,
            computational: delta.computational,
            unverified: delta.unverified,
            restarts: delta.restarts,
            established: delta.established,
            banked: delta.banked,
            solved: delta.solved,
            expired: delta.expired,
        }
    }
}

/// The loop's own transitions.
///
/// Every variant names the pass it belongs to, so a stream can be
/// reconstructed into passes without consulting anything else. The vocabulary
/// covers what neither of the planes underneath can say: which pass this is,
/// which arm won, why the run routed where it did, what the judge scored it,
/// and which bound stopped it.
///
/// The last four variants are the *other* planes joining this one. A
/// [`Recorder`](super::Recorder) registered as a
/// [`RunObserver`](tinyflows::observability::RunObserver) turns the engine's
/// node activations into [`Event::NodeEntered`] and [`Event::NodeFinished`], so
/// a node activation and a model call from the same pass land in one ordered
/// stream rather than in two that have to be reconciled afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A pass round the loop began.
    PassStarted {
        /// The pass that began.
        pass: u32,
    },
    /// A pass round the loop ended, having taken `duration`.
    PassFinished {
        /// The pass that ended.
        pass: u32,
        /// Its wall clock, as the caller measured it.
        duration: Duration,
    },
    /// A step was entered.
    StepEntered {
        /// The pass it belongs to.
        pass: u32,
        /// The step's name.
        step: String,
    },
    /// A step finished, having taken `duration`.
    StepFinished {
        /// The pass it belongs to.
        pass: u32,
        /// The step's name.
        step: String,
        /// How long the step took.
        duration: Duration,
    },
    /// An arm started.
    ArmStarted {
        /// The pass it belongs to.
        pass: u32,
        /// The arm's name.
        arm: String,
    },
    /// An arm finished, having taken `duration`.
    ArmFinished {
        /// The pass it belongs to.
        pass: u32,
        /// The arm's name.
        arm: String,
        /// How long the arm took.
        duration: Duration,
    },
    /// The pass's arms were merged into the accumulator.
    Merged {
        /// The pass it belongs to.
        pass: u32,
        /// How many arms contributed.
        arms: usize,
        /// The summed movement the merge applied.
        movement: Movement,
    },
    /// A judge returned a verdict.
    Judged {
        /// The pass it belongs to.
        pass: u32,
        /// The verdict.
        judgement: Judgement,
        /// The score behind it.
        score: u8,
    },
    /// The run took a route.
    Routed {
        /// The pass it belongs to.
        pass: u32,
        /// The route taken.
        route: Route,
        /// Why it was taken.
        reason: String,
    },
    /// Work was delegated.
    Delegated {
        /// The pass it belongs to.
        pass: u32,
        /// Who it went to.
        to: String,
    },
    /// A delegation came back.
    DelegationFinished {
        /// The pass it belongs to.
        pass: u32,
        /// Who it went to.
        to: String,
        /// How long it took.
        duration: Duration,
    },
    /// An operator told the run something.
    DirectiveReceived {
        /// The pass it belongs to.
        pass: u32,
        /// What they said.
        directive: String,
    },
    /// A note was dropped because the mailbox it was posted to was full.
    ///
    /// The drop is an event rather than a silent loss because the mailbox
    /// exists to keep an aside from stalling the solve: dropping is the
    /// designed behaviour, and a designed loss nobody can see is
    /// indistinguishable from a bug.
    NoteDropped {
        /// The pass it belongs to.
        pass: u32,
        /// Who posted the note that did not fit.
        from: String,
        /// The capacity it did not fit in.
        capacity: usize,
    },
    /// A budget bound tripped.
    BoundTripped {
        /// The pass it belongs to.
        pass: u32,
        /// Which bound.
        bound: Bound,
    },
    /// The run ended.
    LoopFinished {
        /// The last pass.
        pass: u32,
        /// How it came out.
        outcome: Outcome,
    },
    /// A model call completed.
    ModelCalled {
        /// The pass it belongs to.
        pass: u32,
        /// What the provider reported.
        call: ModelCall,
    },
    /// A tool call completed.
    ToolCalled {
        /// The pass it belongs to.
        pass: u32,
        /// What the harness reported.
        call: ToolCall,
    },
    /// The engine activated a node.
    NodeEntered {
        /// The pass it belongs to.
        pass: u32,
        /// The node's id.
        node: String,
    },
    /// The engine's node activation settled.
    NodeFinished {
        /// The pass it belongs to.
        pass: u32,
        /// The node's id.
        node: String,
        /// How long the node's executor ran.
        duration: Duration,
        /// Whether it succeeded.
        ok: bool,
    },
    /// The engine started a graph run.
    EngineRunStarted {
        /// The pass it belongs to.
        pass: u32,
        /// The engine's run id.
        run: String,
    },
    /// The engine's graph run settled.
    EngineRunFinished {
        /// The pass it belongs to.
        pass: u32,
        /// The engine's run id.
        run: String,
        /// How many steps it recorded.
        steps: usize,
        /// The ids of the nodes that errored.
        failed: Vec<String>,
    },
}

impl Event {
    /// The pass this event belongs to.
    #[must_use]
    pub fn pass(&self) -> u32 {
        match *self {
            Self::PassStarted { pass }
            | Self::PassFinished { pass, .. }
            | Self::StepEntered { pass, .. }
            | Self::StepFinished { pass, .. }
            | Self::ArmStarted { pass, .. }
            | Self::ArmFinished { pass, .. }
            | Self::Merged { pass, .. }
            | Self::NoteDropped { pass, .. }
            | Self::Judged { pass, .. }
            | Self::Routed { pass, .. }
            | Self::Delegated { pass, .. }
            | Self::DelegationFinished { pass, .. }
            | Self::DirectiveReceived { pass, .. }
            | Self::BoundTripped { pass, .. }
            | Self::LoopFinished { pass, .. }
            | Self::ModelCalled { pass, .. }
            | Self::ToolCalled { pass, .. }
            | Self::NodeEntered { pass, .. }
            | Self::NodeFinished { pass, .. }
            | Self::EngineRunStarted { pass, .. }
            | Self::EngineRunFinished { pass, .. } => pass,
        }
    }

    /// This event's wire name.
    ///
    /// Hand-written rather than derived from [`Debug`], for the same reason
    /// [`Route::as_str`] is: the string is written into journals that outlive
    /// the process, and a rename must be a compile error here rather than a
    /// quiet change of vocabulary out there.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PassStarted { .. } => "pass_started",
            Self::PassFinished { .. } => "pass_finished",
            Self::StepEntered { .. } => "step_entered",
            Self::StepFinished { .. } => "step_finished",
            Self::ArmStarted { .. } => "arm_started",
            Self::ArmFinished { .. } => "arm_finished",
            Self::Merged { .. } => "merged",
            Self::NoteDropped { .. } => "note_dropped",
            Self::Judged { .. } => "judged",
            Self::Routed { .. } => "routed",
            Self::Delegated { .. } => "delegated",
            Self::DelegationFinished { .. } => "delegation_finished",
            Self::DirectiveReceived { .. } => "directive_received",
            Self::BoundTripped { .. } => "bound_tripped",
            Self::LoopFinished { .. } => "loop_finished",
            Self::ModelCalled { .. } => "model_called",
            Self::ToolCalled { .. } => "tool_called",
            Self::NodeEntered { .. } => "node_entered",
            Self::NodeFinished { .. } => "node_finished",
            Self::EngineRunStarted { .. } => "engine_run_started",
            Self::EngineRunFinished { .. } => "engine_run_finished",
        }
    }

    /// Whether this event is part of the loop's spine.
    ///
    /// The spine — pass boundaries, verdicts, routes, budget trips, and the
    /// ending — reaches *every* filtered view, not only the view of the role
    /// that produced it. Nobody should have to be looking at the right tab to
    /// see that the run changed course, so
    /// [`Recorder::view`](super::Recorder::view) keeps these regardless of
    /// their `who` label.
    #[must_use]
    pub fn is_spine(&self) -> bool {
        matches!(
            self,
            Self::PassStarted { .. }
                | Self::PassFinished { .. }
                | Self::Judged { .. }
                | Self::Routed { .. }
                | Self::BoundTripped { .. }
                | Self::LoopFinished { .. }
        )
    }
}

/// One entry in the run's single ordered stream.
///
/// The `who` label is what makes one stream serviceable as many views: a
/// per-role view is a filter over this, not a second stream that has to be
/// reconciled with the first afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Who produced the event — the recorder's label.
    pub who: String,
    /// What happened.
    pub event: Event,
}

/// Which kind of unit an [`Unpaired`] record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// A step.
    Step,
    /// An arm.
    Arm,
}

/// A step or arm whose entry and completion events did not pair up.
///
/// The regression record for a 62-minute silent gap: a production run printed
/// no driver line for an hour, and which node was holding could only be
/// inferred from which sub-agents happened to spawn during it. "The run
/// stalled" must be a question the log answers, so
/// [`Recorder::unpaired`](super::Recorder::unpaired) makes the missing half of
/// a pair a value a test can assert on rather than an absence nobody notices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unpaired {
    /// The pass it belongs to.
    pub pass: u32,
    /// The step or arm's name.
    pub name: String,
    /// Whether it was a step or an arm.
    pub unit: Unit,
    /// Whether the entry event was seen; `false` means only a completion was.
    pub entered: bool,
}

/// Cumulative spend over some set of model calls.
///
/// Fields are private and read through accessors because they are only ever
/// advanced by [`Self::record`]: a total that could be assigned would be a
/// total nothing derived.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Spend {
    calls: u32,
    prompt_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    cost: Option<f64>,
    unpriced_calls: u32,
    hit_rate_total: f64,
}

impl Spend {
    /// Folds one call into this total.
    ///
    /// A call whose provider reported no cost advances
    /// [`Self::unpriced_calls`] and leaves the money alone. The alternative —
    /// estimating from a local table — would report a number the provider
    /// never said, which is the failure this whole module is arranged against.
    pub fn record(&mut self, call: &ModelCall) {
        self.calls = self.calls.saturating_add(1);
        self.prompt_tokens = self.prompt_tokens.saturating_add(call.prompt_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(call.cached_tokens);
        self.output_tokens = self.output_tokens.saturating_add(call.output_tokens);
        self.hit_rate_total += call.cache_hit_rate;
        match call.cost {
            Some(cost) => self.cost = Some(self.cost.unwrap_or(0.0) + cost),
            None => self.unpriced_calls = self.unpriced_calls.saturating_add(1),
        }
    }

    /// Calls folded in.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls
    }

    /// Prompt tokens.
    #[must_use]
    pub fn prompt_tokens(&self) -> u64 {
        self.prompt_tokens
    }

    /// Prompt tokens served from cache.
    #[must_use]
    pub fn cached_tokens(&self) -> u64 {
        self.cached_tokens
    }

    /// Output tokens.
    #[must_use]
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    /// What the providers said this cost, or `None` when none of them said.
    #[must_use]
    pub fn cost(&self) -> Option<f64> {
        self.cost
    }

    /// Calls whose provider reported no cost.
    ///
    /// Non-zero means [`Self::cost`] is a partial total, which is the honest
    /// reading: the cost/accuracy frontier is simply incomputable for a run
    /// whose provider priced nothing.
    #[must_use]
    pub fn unpriced_calls(&self) -> u32 {
        self.unpriced_calls
    }

    /// The mean prompt-cache hit rate across the calls, or `None` before the
    /// first one.
    #[must_use]
    pub fn cache_hit_rate(&self) -> Option<f64> {
        if self.calls == 0 {
            None
        } else {
            Some(self.hit_rate_total / f64::from(self.calls))
        }
    }
}

/// Per-call accounting, cumulative for the run and split by role and by model.
///
/// Split by *model* as well as by role because the model that answered is not
/// necessarily the one configured: under a fallback ladder a request routes to
/// whatever was available, and a run's spend is only explicable when the split
/// names the models that actually answered.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounting {
    /// Everything the run spent.
    pub run: Spend,
    /// What each role spent.
    pub per_role: BTreeMap<String, Spend>,
    /// What each model that answered cost.
    pub per_model: BTreeMap<String, Spend>,
}

impl Accounting {
    /// Folds one call into the run's total and into its role's and model's.
    pub fn record(&mut self, call: &ModelCall) {
        self.run.record(call);
        self.per_role
            .entry(call.role.clone())
            .or_default()
            .record(call);
        self.per_model
            .entry(call.model.clone())
            .or_default()
            .record(call);
    }
}

/// How long one step took, in which pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTiming {
    /// The pass it belongs to.
    pub pass: u32,
    /// The step's name.
    pub step: String,
    /// How long it took.
    pub duration: Duration,
}

/// Where one pass spent its wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PassProfile {
    /// The pass profiled.
    pub pass: u32,
    /// The pass's wall clock.
    pub wall: Duration,
    /// The summed duration of its steps.
    pub step_time: Duration,
    /// The summed duration of its arms.
    pub arm_time: Duration,
}

impl PassProfile {
    /// Arm time divided by wall clock, or `None` for a pass with no wall clock.
    ///
    /// A **concurrency factor**, not an idle-time figure. Once arms run in
    /// parallel their summed duration legitimately exceeds the pass's wall
    /// clock, so a naive "unaccounted time = wall − work" goes negative and is
    /// then ignored by whoever reads it. A ratio stays meaningful in both
    /// directions: above one is genuine parallelism, below one is a pass that
    /// spent time somewhere other than its arms, and neither is ever negative.
    #[must_use]
    pub fn concurrency_factor(&self) -> Option<f64> {
        if self.wall.is_zero() {
            None
        } else {
            Some(self.arm_time.as_secs_f64() / self.wall.as_secs_f64())
        }
    }
}

/// What a finished run returns.
///
/// One structure serves both the human summary an example prints and the
/// payload a status call answers with, so the observability surface and the
/// control surface cannot diverge: a field a person can see in the summary is a
/// field a caller can read over the bus, and neither can quietly gain something
/// the other lacks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// What the run was trying to achieve.
    pub goal: String,
    /// How the run came out.
    pub outcome: Outcome,
    /// How each attempt came out, oldest first.
    ///
    /// Attempts, not a success bit — see [`Self::reliability`].
    pub attempts: Vec<Outcome>,
    /// The routes taken, in order.
    pub routes: Vec<Route>,
    /// The scores the judge gave, in order.
    pub scores: Vec<u8>,
    /// What the run spent.
    pub spend: Accounting,
    /// Which bound stopped the run, if one did.
    pub bound: Option<Bound>,
    /// How long each step took.
    pub steps: Vec<StepTiming>,
    /// Where each pass spent its wall clock.
    pub passes: Vec<PassProfile>,
    /// What the run did not get to.
    pub undone: Vec<String>,
}

impl Report {
    /// The share of attempts that reached an answer, or `None` before the first
    /// one.
    ///
    /// **Repeat-reliability, not a single success bit.** A 61% single-attempt
    /// pass rate becomes 25% when the same task must be completed across eight
    /// attempts, and capability rankings invert at long horizons: the
    /// configuration that wins on one attempt is not the one that wins on
    /// eight. A lone success boolean reports precisely the number that
    /// inverts, which is why this type does not have one.
    ///
    /// [`Outcome::CleanNoOp`] counts as reaching an answer: the goal was met
    /// and there was legitimately nothing to change.
    #[must_use]
    pub fn reliability(&self) -> Option<f64> {
        if self.attempts.is_empty() {
            return None;
        }
        let reached = self
            .attempts
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::Success | Outcome::CleanNoOp))
            .count();
        Some(ratio(
            u64::try_from(reached).unwrap_or(u64::MAX),
            u64::try_from(self.attempts.len()).unwrap_or(u64::MAX),
        ))
    }

    /// The run, rendered for a person.
    ///
    /// The example prints this and a status call answers with the same
    /// [`Report`] it was built from, which is what keeps the two from drifting.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut lines = vec![
            format!("goal: {}", self.goal),
            format!("outcome: {:?}", self.outcome),
            format!(
                "attempts: {} (reliability {})",
                self.attempts.len(),
                self.reliability()
                    .map_or_else(|| "n/a".to_string(), |value| format!("{value:.2}")),
            ),
            format!(
                "routes: {}",
                self.routes
                    .iter()
                    .map(|route| route.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> "),
            ),
            format!("scores: {:?}", self.scores),
            format!(
                "spend: {} calls, {} prompt + {} output tokens, cost {}",
                self.spend.run.calls(),
                self.spend.run.prompt_tokens(),
                self.spend.run.output_tokens(),
                self.spend
                    .run
                    .cost()
                    .map_or_else(|| "unreported".to_string(), |cost| format!("{cost:.4}")),
            ),
            format!(
                "stopped by: {}",
                self.bound.map_or("nothing", Bound::as_str),
            ),
        ];
        for profile in &self.passes {
            lines.push(format!(
                "pass {}: wall {:?}, steps {:?}, arms {:?}, concurrency {}",
                profile.pass,
                profile.wall,
                profile.step_time,
                profile.arm_time,
                profile
                    .concurrency_factor()
                    .map_or_else(|| "n/a".to_string(), |factor| format!("{factor:.2}")),
            ));
        }
        if !self.undone.is_empty() {
            lines.push(format!("left undone: {}", self.undone.join(", ")));
        }
        lines.join("\n")
    }
}

/// `part / whole` as a fraction, without a lossy integer cast.
///
/// Counts are converted through `u32`, which `f64` represents exactly, so a
/// rate is never quietly wrong for a very long run.
fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let part = f64::from(u32::try_from(part).unwrap_or(u32::MAX));
    let whole = f64::from(u32::try_from(whole).unwrap_or(u32::MAX));
    part / whole
}
