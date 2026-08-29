//! The events a run emits, and who receives them.
//!
//! Three planes already emit something during a loop run, and none of them
//! knows what a loop is. The engine emits node activations through
//! [`RunObserver`](tinyflows::observability::RunObserver); a harness emits
//! model and tool calls with their usage; and a graph diagnosis reports what a
//! green run hid. What none of them can say is *which pass is this, which arm
//! won, why did it route there, what did the judge score it, and which bound
//! stopped the run*. That vocabulary is what this module owns.
//!
//! # One stream, many views
//!
//! [`Recorder`] receives [`Event`]s, tags each with a `who` label, appends it to
//! one journal, and hands it to a [`Sink`]. [`Recorder::child`] returns a view
//! that shares the same journal and the same counters, differing only in its
//! label — so a per-role view is a *filter* over the single stream
//! ([`Recorder::view`]), never a second stream that has to be reconciled with
//! the first afterwards.
//!
//! The recorder implements [`RunObserver`](tinyflows::observability::RunObserver)
//! directly, so the engine's node activations join the same ordered stream. The
//! model-and-tool-call plane arrives through [`CallSink`], a trait defined here
//! rather than by implementing the harness's own `EventListener`: nothing under
//! `src/` may be gated on the optional `tinyagents` dependency, because a host
//! that loads the module must resolve neither it nor its HTTP client. The
//! bridge from that harness onto [`CallSink`] belongs in an example, behind the
//! feature, and an embedder running a different harness implements [`CallSink`]
//! instead of being locked out.
//!
//! # The rules this module asserts rather than recommends
//!
//! - **Every step announces entry and duration.** [`Recorder::unpaired`] makes
//!   a half-reported step a value a test can fail on. See [`Unpaired`] for the
//!   62 minutes of silence that motivates it.
//! - **The loop's spine appears in every view.** [`Event::is_spine`] names the
//!   events [`Recorder::view`] keeps regardless of the label being filtered for.
//! - **Payload-free by default.** [`Capture`] defaults to `false` for both
//!   payload kinds, and [`Recorder::record`] strips what was not opted into
//!   *before* the event reaches the journal or any sink.
//! - **Prompt-cache hit rate is per call**, a field of [`ModelCall`] rather
//!   than something derived from token counts later.
//! - **No observability call blocks the loop.** A sink that cannot write drops
//!   the entry and counts the drop; see [`JsonlSink::drops`].
//!
//! # The clock is the caller's
//!
//! Nothing here reads the wall clock. Durations arrive on the events that carry
//! them, which is what lets a test assert on a run's timing profile at all.

use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tinyflows::observability::{ExecutionStep, Run, RunObserver, StepStatus};

mod types;

pub use types::{
    Accounting, Capture, Entry, Event, ModelCall, Movement, PassProfile, Report, Spend, StepTiming,
    ToolCall, Unit, Unpaired,
};

use crate::policy::Outcome;

/// Receives the loop's events.
///
/// Implementations must be cheap and must not fail loudly: a sink runs inline
/// with the loop, so anything it cannot do it drops and counts. The trait takes
/// `&self` and requires `Send + Sync` because one sink is shared by every view
/// of a run.
pub trait Sink: Send + Sync {
    /// Receives one event.
    fn emit(&self, event: &Event);
}

/// Receives the model and tool calls a harness reports.
///
/// The seam the third plane arrives on. It exists as a local trait rather than
/// as an implementation of a specific harness's listener so that nothing under
/// `src/` depends on that harness — see the module docs.
pub trait CallSink: Send + Sync {
    /// Records one completed model call.
    fn model_call(&self, pass: u32, call: ModelCall);

    /// Records one completed tool call.
    fn tool_call(&self, pass: u32, call: ToolCall);
}

/// Locks `mutex`, taking the value back from a poisoned lock.
///
/// A panic in one sink must not silently stop the run's journal from being
/// written: the data behind the lock is a `Vec` of records, and a partially
/// appended `Vec` is still the run's history. Recovering is strictly better
/// than the alternatives, which are propagating a panic out of an observability
/// call or dropping every subsequent event.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One event, rendered as a single line.
///
/// One line per event, and one line per node per pass on entry and completion,
/// because "the run stalled" must be a question the log answers rather than one
/// a reader answers by correlating which sub-agents happened to spawn during a
/// gap.
#[must_use]
pub fn render(event: &Event) -> String {
    let pass = event.pass();
    match event {
        Event::PassStarted { .. } => format!("pass {pass} started"),
        Event::PassFinished { duration, .. } => format!("pass {pass} finished in {duration:?}"),
        Event::StepEntered { step, .. } => format!("pass {pass} step {step} entered"),
        Event::StepFinished { step, duration, .. } => {
            format!("pass {pass} step {step} finished in {duration:?}")
        }
        Event::NoteDropped { from, capacity, .. } => {
            format!("pass {pass} note from {from} dropped, mailbox full at {capacity}")
        }
        Event::Amended {
            revision,
            change,
            because,
            ..
        } => format!("pass {pass} amended to r{revision}: {change} — {because}"),
        Event::AmendmentRefused { change, reason, .. } => {
            format!("pass {pass} refused {change}: {reason}")
        }
        Event::ArmStarted { arm, .. } => format!("pass {pass} arm {arm} started"),
        Event::ArmFinished { arm, duration, .. } => {
            format!("pass {pass} arm {arm} finished in {duration:?}")
        }
        Event::Merged { arms, movement, .. } => {
            format!(
                "pass {pass} merged {arms} arms: attempts {:+}, established {:+}, banked {:+}",
                movement.attempts, movement.established, movement.banked,
            )
        }
        Event::Judged {
            judgement, score, ..
        } => format!("pass {pass} judged {} score {score}", judgement.as_str()),
        Event::Routed { route, reason, .. } => {
            format!("pass {pass} routed {} because {reason}", route.as_str())
        }
        Event::Delegated { to, .. } => format!("pass {pass} delegated to {to}"),
        Event::DelegationFinished { to, duration, .. } => {
            format!("pass {pass} delegation to {to} finished in {duration:?}")
        }
        Event::DirectiveReceived { directive, .. } => {
            format!("pass {pass} directive: {directive}")
        }
        Event::BoundTripped { bound, .. } => {
            format!("pass {pass} stopped by {}", bound.as_str())
        }
        Event::LoopFinished { outcome, .. } => format!("pass {pass} loop finished {outcome:?}"),
        Event::ModelCalled { call, .. } => format!(
            "pass {pass} model {}/{} for {}: {} prompt ({} cached, {:.0}% hit), {} output",
            call.provider,
            call.model,
            call.role,
            call.prompt_tokens,
            call.cached_tokens,
            call.cache_hit_rate * 100.0,
            call.output_tokens,
        ),
        Event::ToolCalled { call, .. } => format!(
            "pass {pass} tool {} for {} took {:?}{}",
            call.tool,
            call.role,
            call.duration,
            if call.timed_out { " (timed out)" } else { "" },
        ),
        Event::NodeEntered { node, .. } => format!("pass {pass} node {node} entered"),
        Event::NodeFinished {
            node, duration, ok, ..
        } => format!(
            "pass {pass} node {node} finished in {duration:?} ({})",
            if *ok { "ok" } else { "error" },
        ),
        Event::EngineRunStarted { run, .. } => format!("pass {pass} engine run {run} started"),
        Event::EngineRunFinished {
            run, steps, failed, ..
        } => format!(
            "pass {pass} engine run {run} finished: {steps} steps, {} failed",
            failed.len(),
        ),
    }
}

/// A [`Sink`] that broadcasts to several others, in registration order.
///
/// Fan-out is best-effort in the same sense the sinks are: one sink dropping an
/// entry does not stop the next from receiving it.
#[derive(Clone, Default)]
pub struct FanOutSink {
    sinks: Vec<Arc<dyn Sink>>,
}

impl std::fmt::Debug for FanOutSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanOutSink")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl FanOutSink {
    /// An empty fan-out, which drops everything until something is added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a sink to the end of the fan-out.
    #[must_use]
    pub fn with(mut self, sink: Arc<dyn Sink>) -> Self {
        self.sinks.push(sink);
        self
    }

    /// How many sinks are installed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// Whether no sink is installed, in which case every event is dropped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl Sink for FanOutSink {
    fn emit(&self, event: &Event) {
        for sink in &self.sinks {
            sink.emit(event);
        }
    }
}

/// A [`Sink`] that writes one rendered line per event.
///
/// The console renderer. Takes any writer so a test can read back exactly what
/// a terminal would have shown.
pub struct LineSink {
    out: Mutex<Box<dyn Write + Send>>,
    drops: AtomicU64,
}

impl std::fmt::Debug for LineSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The writer is not renderable, so the counter that matters is.
        f.debug_struct("LineSink")
            .field("drops", &self.drops.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LineSink {
    /// Writes rendered lines to `writer`.
    pub fn new<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            out: Mutex::new(Box::new(writer)),
            drops: AtomicU64::new(0),
        }
    }

    /// Writes rendered lines to standard output.
    #[must_use]
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }

    /// How many entries this sink failed to write.
    ///
    /// A sink that cannot write drops the entry rather than failing the run,
    /// and counts it rather than losing it silently.
    #[must_use]
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
}

impl Sink for LineSink {
    fn emit(&self, event: &Event) {
        let line = render(event);
        let mut out = lock(&self.out);
        if writeln!(out, "{line}").is_err() {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A [`Sink`] that writes each event as one JSON line.
///
/// The durable shape. Serialization and the write are both best-effort: an
/// entry that cannot be written is dropped and counted, because no
/// observability call may block or fail the loop.
pub struct JsonlSink {
    out: Mutex<Box<dyn Write + Send>>,
    drops: AtomicU64,
}

impl std::fmt::Debug for JsonlSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The writer is not renderable, so the counter that matters is.
        f.debug_struct("JsonlSink")
            .field("drops", &self.drops.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl JsonlSink {
    /// Writes JSON lines to `writer`.
    pub fn new<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            out: Mutex::new(Box::new(writer)),
            drops: AtomicU64::new(0),
        }
    }

    /// How many entries this sink failed to serialize or write.
    #[must_use]
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
}

impl Sink for JsonlSink {
    fn emit(&self, event: &Event) {
        let Ok(line) = serde_json::to_string(event) else {
            self.drops.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let mut out = lock(&self.out);
        if writeln!(out, "{line}").is_err() {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A [`Sink`] that masks known secrets before forwarding to another.
///
/// Redaction is generic rather than field-aware: the event is serialized, every
/// string at any depth has each secret replaced by the mask, and the result is
/// deserialized back. A field added later is therefore covered without anybody
/// remembering to cover it.
///
/// **An event that cannot be redacted is dropped, not forwarded.** Forwarding
/// the original would mean the one entry whose redaction failed is the one
/// entry that reaches the log verbatim, which inverts the sink's purpose;
/// [`Self::drops`] makes the loss visible instead.
pub struct RedactingSink {
    inner: Arc<dyn Sink>,
    secrets: Vec<String>,
    mask: String,
    drops: AtomicU64,
}

impl std::fmt::Debug for RedactingSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secrets themselves are deliberately not rendered: a `Debug` line
        // that prints them would leak exactly what the sink exists to hide.
        f.debug_struct("RedactingSink")
            .field("secrets", &self.secrets.len())
            .field("mask", &self.mask)
            .field("drops", &self.drops.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl RedactingSink {
    /// Masks every occurrence of `secrets` with `[redacted]` before forwarding
    /// to `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn Sink>, secrets: Vec<String>) -> Self {
        Self::with_mask(inner, secrets, "[redacted]")
    }

    /// Masks with a caller-chosen replacement.
    #[must_use]
    pub fn with_mask(inner: Arc<dyn Sink>, secrets: Vec<String>, mask: impl Into<String>) -> Self {
        Self {
            inner,
            // An empty secret would match everywhere and mask the whole stream,
            // so it is dropped rather than honoured.
            secrets: secrets.into_iter().filter(|s| !s.is_empty()).collect(),
            mask: mask.into(),
            drops: AtomicU64::new(0),
        }
    }

    /// How many entries could not be redacted and were dropped.
    #[must_use]
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }

    /// Replaces every secret in every string of `value`, at any depth.
    fn scrub(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                for secret in &self.secrets {
                    if text.contains(secret.as_str()) {
                        *text = text.replace(secret.as_str(), &self.mask);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.scrub(item);
                }
            }
            serde_json::Value::Object(fields) => {
                for (_, field) in fields.iter_mut() {
                    self.scrub(field);
                }
            }
            _ => {}
        }
    }
}

impl Sink for RedactingSink {
    fn emit(&self, event: &Event) {
        let Ok(mut value) = serde_json::to_value(event) else {
            self.drops.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.scrub(&mut value);
        match serde_json::from_value::<Event>(value) {
            Ok(redacted) => self.inner.emit(&redacted),
            Err(_) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// What every view of one run shares: the sink, the journal, and the counters.
struct Shared {
    sink: Arc<dyn Sink>,
    capture: Capture,
    journal: Mutex<Vec<Entry>>,
    accounting: Mutex<Accounting>,
    pass: AtomicU32,
}

/// Receives a run's events, tags them, and derives what the run reports.
///
/// A recorder is one view of a run. [`Self::child`] makes another with a
/// different label over the *same* journal and the same counters, so a
/// delegated run gets its own view without a second writer to reconcile —
/// entries from `child("judge")` and its parent appear in one journal, and the
/// parent's accounting includes the child's.
///
/// The recorder is also the engine's
/// [`RunObserver`](tinyflows::observability::RunObserver) and the harness's
/// [`CallSink`], which is what folds three planes into one ordered stream.
#[derive(Clone)]
pub struct Recorder {
    who: String,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("who", &self.who)
            .field("capture", &self.shared.capture)
            .field("entries", &lock(&self.shared.journal).len())
            .finish()
    }
}

impl Recorder {
    /// A recorder labelled `who`, delivering to `sink`, capturing nothing.
    ///
    /// Capturing nothing is the default everywhere in this module. A run that
    /// should record prompts says so with [`Self::with_capture`].
    #[must_use]
    pub fn new(who: impl Into<String>, sink: Arc<dyn Sink>) -> Self {
        Self::with_capture(who, sink, Capture::default())
    }

    /// A recorder that captures what `capture` allows.
    #[must_use]
    pub fn with_capture(who: impl Into<String>, sink: Arc<dyn Sink>, capture: Capture) -> Self {
        Self {
            who: who.into(),
            shared: Arc::new(Shared {
                sink,
                capture,
                journal: Mutex::new(Vec::new()),
                accounting: Mutex::new(Accounting::default()),
                pass: AtomicU32::new(0),
            }),
        }
    }

    /// Another view of the same run, labelled `label`.
    ///
    /// Shares the journal, the accounting, and the current pass. Only the label
    /// differs, which is the whole design: one stream, filtered many ways.
    #[must_use]
    pub fn child(&self, label: impl Into<String>) -> Self {
        Self {
            who: label.into(),
            shared: Arc::clone(&self.shared),
        }
    }

    /// This view's label.
    #[must_use]
    pub fn who(&self) -> &str {
        &self.who
    }

    /// What this run captures.
    #[must_use]
    pub fn capture(&self) -> Capture {
        self.shared.capture
    }

    /// The pass the run is currently in.
    ///
    /// Set by [`Event::PassStarted`], and read by the callbacks that have no
    /// pass of their own — the engine reports node activations, not passes, so
    /// this is how its entries land in the right one.
    #[must_use]
    pub fn current_pass(&self) -> u32 {
        self.shared.pass.load(Ordering::Relaxed)
    }

    /// Records one event: strips payloads, journals it, and emits it.
    ///
    /// Capture is applied *here*, before the journal and before the sink, so an
    /// un-opted-into payload does not exist downstream rather than merely going
    /// unrendered. That placement is what makes "with capture off, no prompt
    /// text reaches any sink" a property of the recorder rather than of every
    /// sink separately.
    pub fn record(&self, event: Event) {
        let event = self.apply_capture(event);

        if let Event::PassStarted { pass } = event {
            self.shared.pass.store(pass, Ordering::Relaxed);
        }
        if let Event::ModelCalled { ref call, .. } = event {
            lock(&self.shared.accounting).record(call);
        }

        lock(&self.shared.journal).push(Entry {
            who: self.who.clone(),
            event: event.clone(),
        });
        self.shared.sink.emit(&event);
    }

    /// Drops any payload this run did not opt into capturing.
    fn apply_capture(&self, event: Event) -> Event {
        match event {
            Event::ModelCalled { pass, call } if !self.shared.capture.model_io => {
                Event::ModelCalled {
                    pass,
                    call: call.without_payloads(),
                }
            }
            Event::ToolCalled { pass, call } if !self.shared.capture.tool_io => Event::ToolCalled {
                pass,
                call: call.without_payloads(),
            },
            other => other,
        }
    }

    /// Every entry, in order.
    #[must_use]
    pub fn journal(&self) -> Vec<Entry> {
        lock(&self.shared.journal).clone()
    }

    /// The entries one label produced, plus the whole run's spine.
    ///
    /// The spine is included on purpose: pass boundaries, verdicts, routes, and
    /// budget trips reach every filtered view, so nobody has to be looking at
    /// the right tab to see that the run changed course.
    #[must_use]
    pub fn view(&self, who: &str) -> Vec<Entry> {
        lock(&self.shared.journal)
            .iter()
            .filter(|entry| entry.who == who || entry.event.is_spine())
            .cloned()
            .collect()
    }

    /// What the run has spent, for the run and split by role and model.
    #[must_use]
    pub fn accounting(&self) -> Accounting {
        lock(&self.shared.accounting).clone()
    }

    /// Steps and arms whose entry and completion did not pair up.
    ///
    /// Empty for a healthy run. A non-empty result names a unit that started
    /// and never reported finishing — or, rarer and just as wrong, finished
    /// without having announced itself.
    #[must_use]
    pub fn unpaired(&self) -> Vec<Unpaired> {
        let journal = lock(&self.shared.journal);
        let mut open: Vec<(u32, String, Unit)> = Vec::new();
        let mut orphans: Vec<Unpaired> = Vec::new();

        for entry in journal.iter() {
            let (pass, name, unit, entering) = match &entry.event {
                Event::StepEntered { pass, step } => (*pass, step.clone(), Unit::Step, true),
                Event::StepFinished { pass, step, .. } => (*pass, step.clone(), Unit::Step, false),
                Event::ArmStarted { pass, arm } => (*pass, arm.clone(), Unit::Arm, true),
                Event::ArmFinished { pass, arm, .. } => (*pass, arm.clone(), Unit::Arm, false),
                _ => continue,
            };

            if entering {
                open.push((pass, name, unit));
            } else if let Some(index) = open
                .iter()
                .position(|(p, n, u)| *p == pass && n == &name && *u == unit)
            {
                open.remove(index);
            } else {
                orphans.push(Unpaired {
                    pass,
                    name,
                    unit,
                    entered: false,
                });
            }
        }

        orphans.extend(open.into_iter().map(|(pass, name, unit)| Unpaired {
            pass,
            name,
            unit,
            entered: true,
        }));
        orphans
    }

    /// The run's report, derived from the journal.
    ///
    /// `attempts` is the outcome of each repeat, oldest first, because a run
    /// reports how it fared across repeats rather than whether one attempt
    /// passed — see [`Report::reliability`]. `undone` is what the run did not
    /// get to.
    #[must_use]
    pub fn report(
        &self,
        goal: impl Into<String>,
        outcome: Outcome,
        attempts: Vec<Outcome>,
        undone: Vec<String>,
    ) -> Report {
        let journal = lock(&self.shared.journal);
        let mut report = Report {
            goal: goal.into(),
            outcome,
            attempts,
            routes: Vec::new(),
            scores: Vec::new(),
            spend: lock(&self.shared.accounting).clone(),
            bound: None,
            steps: Vec::new(),
            passes: Vec::new(),
            undone,
        };

        for entry in journal.iter() {
            match &entry.event {
                Event::Routed { route, .. } => report.routes.push(*route),
                Event::Judged { score, .. } => report.scores.push(*score),
                Event::BoundTripped { bound, .. } => report.bound = Some(*bound),
                Event::StepFinished {
                    pass,
                    step,
                    duration,
                } => report.steps.push(StepTiming {
                    pass: *pass,
                    step: step.clone(),
                    duration: *duration,
                }),
                _ => {}
            }

            let profile = profile_for(&mut report.passes, entry.event.pass());
            match &entry.event {
                Event::PassFinished { duration, .. } => {
                    profile.wall = profile.wall.saturating_add(*duration);
                }
                Event::StepFinished { duration, .. } => {
                    profile.step_time = profile.step_time.saturating_add(*duration);
                }
                Event::ArmFinished { duration, .. } => {
                    profile.arm_time = profile.arm_time.saturating_add(*duration);
                }
                _ => {}
            }
        }

        report
    }
}

/// The profile for `pass`, appending one if this is the first sight of it.
fn profile_for(profiles: &mut Vec<PassProfile>, pass: u32) -> &mut PassProfile {
    if let Some(index) = profiles.iter().position(|profile| profile.pass == pass) {
        return &mut profiles[index];
    }
    profiles.push(PassProfile {
        pass,
        ..PassProfile::default()
    });
    // The push above guarantees a last element; `else` is unreachable rather
    // than merely unlikely, and returning a fresh default there would hand back
    // a profile nothing accumulates into.
    let index = profiles.len() - 1;
    &mut profiles[index]
}

impl CallSink for Recorder {
    fn model_call(&self, pass: u32, call: ModelCall) {
        self.record(Event::ModelCalled { pass, call });
    }

    fn tool_call(&self, pass: u32, call: ToolCall) {
        self.record(Event::ToolCalled { pass, call });
    }
}

/// The engine's node activations, folded into the loop's own stream.
///
/// The engine knows nodes, not passes, so each callback is stamped with
/// [`Recorder::current_pass`] — the pass the loop last announced. That is what
/// puts a node activation and a model call from the same pass in their true
/// order in one journal, each with its own `who` label.
impl RunObserver for Recorder {
    fn on_run_start(&self, run_id: &str) {
        self.record(Event::EngineRunStarted {
            pass: self.current_pass(),
            run: run_id.to_string(),
        });
    }

    fn on_step_start(&self, node_id: &str) {
        self.record(Event::NodeEntered {
            pass: self.current_pass(),
            node: node_id.to_string(),
        });
    }

    fn on_step_finish(&self, step: &ExecutionStep) {
        self.record(Event::NodeFinished {
            pass: self.current_pass(),
            node: step.node_id.clone(),
            // The engine reports milliseconds as a `u128`; saturating keeps a
            // nonsense duration from panicking an observability call.
            duration: Duration::from_millis(u64::try_from(step.duration_ms).unwrap_or(u64::MAX)),
            ok: !matches!(step.status, StepStatus::Error),
        });
    }

    fn on_run_finish(&self, run: &Run) {
        self.record(Event::EngineRunFinished {
            pass: self.current_pass(),
            run: run.id.clone(),
            steps: run.steps.len(),
            failed: run
                .failed_node_ids()
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        });
    }
}

#[cfg(test)]
mod test;
