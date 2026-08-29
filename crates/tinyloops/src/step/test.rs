//! Unit tests for the closed step set and the one tool it is reached through.
//!
//! Three things are pinned here, and each because its failure is quiet:
//!
//! - **the set is closed** — an unknown step name is an error, never a no-op,
//!   because a no-op leaves the run to route on a state nobody advanced;
//! - **a malformed payload is refused** — falling back to a default step or a
//!   default accumulator is the same failure wearing a different hat;
//! - **the whole state comes back** — the head replaces its slot with what the
//!   tool returned, so a step that dropped a field would silently revert it.
//!
//! Invariant 11 — that an observing body cannot emit an accumulator — is proved
//! by the two `compile_fail` doctests on [`Observer`], not here: a runtime
//! assertion is exactly the check the type system was brought in to replace.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::*;
use crate::policy::{Judgement, Thresholds};

/// A step that stamps the pass number it was handed onto `attempts`.
///
/// An assignment rather than an increment, so replaying it twice is the same as
/// running it once.
struct Attempt;

impl Step for Attempt {
    fn name(&self) -> &'static str {
        STEP_ATTEMPT
    }

    fn run(&self, mut state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        state.attempts = ctx.pass() + 1;
        state.last_attempt = format!("pass {}", ctx.pass());
        Ok(ctx.advance(state))
    }
}

/// A step that fails, so the failure path is covered by a body rather than only
/// by the dispatcher.
struct Broken;

impl Step for Broken {
    fn name(&self) -> &'static str {
        "broken"
    }

    fn run(&self, _state: LoopState, _ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        Err(Error::EmptyName)
    }
}

/// An observer that records how many times it ran, and what it saw.
struct Counting {
    seen: std::sync::Mutex<Vec<u32>>,
}

impl Observer for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }

    fn observe(&self, state: &LoopState, ctx: StepContext<'_, NoWrite>) -> Result<()> {
        self.seen.lock().unwrap().push(ctx.pass());
        assert_eq!(state.passes, ctx.pass());
        Ok(())
    }
}

/// A failing observer, so an observing body's error is proved to reach the
/// caller rather than being swallowed by the unchanged-state wrapping.
struct BrokenObserver;

impl Observer for BrokenObserver {
    fn name(&self) -> &'static str {
        "broken_observer"
    }

    fn observe(&self, _state: &LoopState, _ctx: StepContext<'_, NoWrite>) -> Result<()> {
        Err(Error::EmptyName)
    }
}

fn registry() -> StepRegistry {
    let mut registry = StepRegistry::new();
    registry.register(Arc::new(Attempt)).unwrap();
    registry
}

#[test]
fn resolves_a_registered_step_by_name() {
    let registry = registry();
    let entry = registry.get(STEP_ATTEMPT).unwrap();

    assert_eq!(entry.name(), STEP_ATTEMPT);
    assert!(entry.advances());
}

#[test]
fn rejects_an_unregistered_step_by_name() {
    let registry = registry();

    assert_eq!(
        registry.get("attmept").unwrap_err(),
        Error::UnknownStep {
            name: "attmept".to_string()
        },
    );
}

#[test]
fn rejects_a_second_registration_of_the_same_name() {
    let mut registry = registry();

    assert_eq!(
        registry.register(Arc::new(Attempt)).unwrap_err(),
        Error::DuplicateStep {
            name: STEP_ATTEMPT.to_string()
        },
    );
}

#[test]
fn rejects_an_observer_registered_under_a_taken_name() {
    let mut registry = StepRegistry::new();
    registry
        .register_observer(Arc::new(Counting {
            seen: std::sync::Mutex::new(Vec::new()),
        }))
        .unwrap();

    assert_eq!(
        registry
            .register_observer(Arc::new(Counting {
                seen: std::sync::Mutex::new(Vec::new()),
            }))
            .unwrap_err(),
        Error::DuplicateStep {
            name: "counting".to_string()
        },
    );
}

#[test]
fn reports_the_registered_names_in_a_stable_order() {
    let mut registry = registry();
    registry.register(Arc::new(Broken)).unwrap();
    registry
        .register_observer(Arc::new(Counting {
            seen: std::sync::Mutex::new(Vec::new()),
        }))
        .unwrap();

    // Sorted, not insertion-ordered: the graph builder reads this list and must
    // emit the same bytes for the same inputs.
    assert_eq!(registry.names(), ["attempt", "broken", "counting"]);
    assert_eq!(registry.len(), 3);
    assert!(!registry.is_empty());
    assert!(StepRegistry::new().is_empty());
}

#[test]
fn runs_the_named_step_and_returns_the_whole_state() {
    let registry = registry();
    let mut state = LoopState::new("goal");
    state.passes = 2;
    state.lessons.push("something learned".to_string());

    let returned = registry.run(STEP_ATTEMPT, state).unwrap();

    assert_eq!(returned.attempts, 3);
    assert_eq!(returned.last_attempt, "pass 2");
    // The narrative the step did not touch came back untouched rather than
    // absent: the head replaces its slot with this value wholesale.
    assert_eq!(returned.lessons, ["something learned"]);
    assert_eq!(returned.goal, "goal");
}

#[test]
fn an_observing_step_returns_the_state_it_was_handed() {
    let observer = Arc::new(Counting {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let mut registry = StepRegistry::new();
    registry.register_observer(observer.clone()).unwrap();

    let mut state = LoopState::new("goal");
    state.passes = 4;
    state.judged = Judgement::Steer;

    let returned = registry.run("counting", state.clone()).unwrap();

    assert_eq!(returned, state);
    assert_eq!(*observer.seen.lock().unwrap(), [4]);
}

#[test]
fn a_failing_step_reports_rather_than_returning_the_state_unchanged() {
    let mut registry = StepRegistry::new();
    registry.register(Arc::new(Broken)).unwrap();

    assert_eq!(
        registry.run("broken", LoopState::new("goal")).unwrap_err(),
        Error::EmptyName,
    );
}

#[test]
fn a_failing_observer_reports_rather_than_being_swallowed() {
    let mut registry = StepRegistry::new();
    registry
        .register_observer(Arc::new(BrokenObserver))
        .unwrap();

    assert_eq!(
        registry
            .run("broken_observer", LoopState::new("goal"))
            .unwrap_err(),
        Error::EmptyName,
    );
}

#[test]
fn the_context_carries_the_thresholds_in_force() {
    let thresholds = Thresholds {
        stuck: 9,
        ..Thresholds::default()
    };
    let ctx = StepContext::advancing(1, &thresholds);

    assert_eq!(ctx.pass(), 1);
    assert_eq!(ctx.thresholds().stuck, 9);
    assert_eq!(StepContext::observing(7, &thresholds).pass(), 7);
}

#[test]
fn the_tool_runs_the_named_step_and_returns_the_state_as_json() {
    let registry = registry();
    let mut state = LoopState::new("goal");
    state.passes = 1;

    let returned =
        run_loop_step(&registry, &json!({ "step": STEP_ATTEMPT, "state": state })).unwrap();

    assert_eq!(returned["attempts"], json!(2));
    assert_eq!(returned["goal"], json!("goal"));
    // Every field of the accumulator is present, because the head replaces its
    // slot with exactly this value. The count is spelled out rather than
    // derived so that adding a field to `LoopState` without deciding how the
    // head folds it fails here.
    assert_eq!(returned.as_object().unwrap().len(), 21);
}

#[test]
fn the_tool_rejects_an_unknown_step_name() {
    let registry = registry();

    assert_eq!(
        run_loop_step(
            &registry,
            &json!({ "step": "nope", "state": LoopState::new("goal") }),
        )
        .unwrap_err(),
        Error::UnknownStep {
            name: "nope".to_string()
        },
    );
}

#[test]
fn the_tool_rejects_a_payload_with_no_step_name() {
    let registry = registry();

    assert_eq!(
        run_loop_step(&registry, &json!({ "state": LoopState::new("goal") }),).unwrap_err(),
        Error::MalformedStepPayload { field: "step" },
    );
}

#[test]
fn the_tool_rejects_a_step_name_that_is_not_a_string() {
    let registry = registry();

    assert_eq!(
        run_loop_step(
            &registry,
            &json!({ "step": 7, "state": LoopState::new("goal") }),
        )
        .unwrap_err(),
        Error::MalformedStepPayload { field: "step" },
    );
}

#[test]
fn the_tool_rejects_a_payload_with_no_state() {
    let registry = registry();

    assert_eq!(
        run_loop_step(&registry, &json!({ "step": "attempt" })).unwrap_err(),
        Error::MalformedStepPayload { field: "state" },
    );
}

#[test]
fn the_tool_rejects_a_state_that_is_not_an_accumulator() {
    let registry = registry();

    assert_eq!(
        run_loop_step(
            &registry,
            &json!({ "step": "attempt", "state": { "passes": "many" } }),
        )
        .unwrap_err(),
        Error::MalformedStepPayload { field: "state" },
    );
}

#[test]
fn the_slug_list_and_the_constants_agree() {
    // One list, so a graph builder and a registry cannot spell a step
    // differently. Asserted rather than assumed because both failures are
    // quiet: a name only the builder knows errors at the first invocation, and
    // a name only the registry knows is a body that is never run.
    assert_eq!(STEP_NAMES.len(), 7);
    assert!(STEP_NAMES.contains(&STEP_PLAN));
    assert!(STEP_NAMES.contains(&STEP_RESEARCH));
    assert!(STEP_NAMES.contains(&STEP_ATTEMPT));
    assert!(STEP_NAMES.contains(&STEP_REFLECT));
    assert!(STEP_NAMES.contains(&STEP_JUDGE));
    assert!(STEP_NAMES.contains(&STEP_PASS));
    assert!(STEP_NAMES.contains(&STEP_REPORT));
    assert_eq!(RUN_LOOP_STEP, "run_loop_step");
}

#[test]
fn an_advanced_state_can_be_read_and_taken() {
    let thresholds = Thresholds::default();
    let ctx = StepContext::advancing(0, &thresholds);
    let advanced = ctx.advance(LoopState::new("goal"));

    assert_eq!(advanced.state().goal, "goal");
    assert_eq!(advanced.into_state().goal, "goal");
}

#[test]
fn debug_rendering_names_the_registered_steps() {
    let registry = registry();
    let rendered = format!("{registry:?}");

    assert!(rendered.contains("attempt"));
    assert!(format!("{:?}", registry.get(STEP_ATTEMPT).unwrap()).contains("advances: true"));
}

/// A step that records the thresholds its `StepContext` was actually handed,
/// rather than anything it could read back off the returned state.
struct Watching {
    seen: std::sync::Mutex<Vec<Thresholds>>,
}

impl Step for Watching {
    fn name(&self) -> &'static str {
        "watching"
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        self.seen.lock().unwrap().push(*ctx.thresholds());
        Ok(ctx.advance(state))
    }
}

#[test]
fn a_step_is_handed_the_thresholds_its_state_carries() {
    // The seam reads the thresholds off the state rather than from a caller:
    // a body handed a threshold set the run is not using would route on
    // numbers nobody configured, and nothing would report it. Asserted on
    // what the context handed the step, not on the state the step returned —
    // a body that never reads `ctx.thresholds()` at all would still pass an
    // assertion against the returned profile, since `Attempt` here (and the
    // registry's other bodies) carry the profile through untouched.
    let watching = Arc::new(Watching {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let mut registry = StepRegistry::new();
    registry.register(watching.clone()).unwrap();
    let profile = crate::LoopProfile::of(crate::Preset::Persistent);
    let state = LoopState::with_profile("goal", profile.clone());

    let returned = registry.run("watching", state).unwrap();

    assert_eq!(
        watching.seen.lock().unwrap().as_slice(),
        [profile.thresholds],
        "the step was not handed the thresholds its state carries",
    );
    assert_eq!(returned.profile.thresholds.stuck, 4);
}
