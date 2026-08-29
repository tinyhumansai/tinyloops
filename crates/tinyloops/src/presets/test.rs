//! Unit tests for the batteries: the presets and their bets, the two arms'
//! anti-confabulation rules, and an assembled loop driven end to end.
//!
//! The arm tests are the load-bearing ones. Each names the failure it prevents,
//! because "a `SOLVED` without an artifact does not end the loop" is not a
//! detail of an implementation — it is the one control this design has over a
//! verifier that is itself a model.

// Tests may panic on a broken invariant; that is the assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    AssembledLoop, Judge, Preset, Reflect, Rules, SOLVED_MARKER, research_loop, tuned_research_loop,
};
use crate::arm::{Arm, ArmSet};
use crate::budget::{Bound, Caps, RunBudget};
use crate::error::Error;
use crate::harness::{Artifact, Scripted};
use crate::observe::{Event, LineSink, Recorder, Sink};
use crate::orchestrate::{AttemptReport, DelegateSet, FixedPlan, Inline};
use crate::policy::{Autonomy, Judgement, Outcome, Route, Thresholds, evaluate_ladder};
use crate::state::{Contribution, LoopState};
use crate::step::{NoWrite, StepContext};

fn quiet() -> Recorder {
    Recorder::new("test", Arc::new(LineSink::new(std::io::sink())))
}

fn observing(thresholds: &Thresholds) -> StepContext<'_, NoWrite> {
    StepContext::observing(0, thresholds)
}

// --------------------------------------------------------------- the presets

#[test]
fn every_preset_names_itself_and_reads_back() {
    for preset in Preset::ALL {
        assert_eq!(Preset::parse(preset.as_str()), Some(preset));
        assert_eq!(preset.to_string(), preset.as_str());
    }
    assert_eq!(Preset::parse("invented"), None);
    assert_eq!(Preset::ALL.len(), 4);
}

#[test]
fn each_preset_deviates_from_the_default_in_the_field_its_bet_is_about() {
    // A preset whose numbers match the default is a preset with a rationale and
    // no behavior, which is worse than not shipping it: it reads like a choice
    // and changes nothing.
    let balanced = Preset::Balanced.thresholds();

    assert_eq!(balanced, Thresholds::default());
    assert!(Preset::Persistent.thresholds().stuck > balanced.stuck);
    assert!(Preset::Persistent.thresholds().max_attempts > balanced.max_attempts);
    assert!(Preset::Exploratory.thresholds().stuck < balanced.stuck);
    assert!(Preset::Exploratory.thresholds().computational < balanced.computational);
    assert!(Preset::Cautious.thresholds().unverified < balanced.unverified);
}

#[test]
fn the_persistence_bet_and_the_variation_bet_route_the_same_state_differently() {
    // The same run, one pass into a stall, read by two presets. This is the bet
    // made visible: exploratory diversifies, persistent keeps revising.
    let mut exploratory =
        LoopState::with_profile("goal", crate::policy::LoopProfile::of(Preset::Exploratory));
    exploratory.unproductive = 1;
    let mut persistent =
        LoopState::with_profile("goal", crate::policy::LoopProfile::of(Preset::Persistent));
    persistent.unproductive = 1;

    assert_eq!(crate::policy::route(&exploratory), Route::Diversify);
    assert_eq!(crate::policy::route(&persistent), Route::Retry);
}

#[test]
fn every_preset_builds_a_graph_that_validates_and_compiles() {
    for preset in Preset::ALL {
        let assembled = assembled(preset).expect("the preset assembles");
        let graph = assembled.graph().expect("the graph validates");

        assert!(!graph.nodes.is_empty());
        tinyflows::compiler::compile(&graph)
            .unwrap_or_else(|error| panic!("{preset} did not compile: {error}"));
    }
}

#[test]
fn the_presets_are_the_set_the_parity_sweep_reads() {
    // The sweep in `src/policy/test.rs` iterates `Preset::ALL`. Asserting the
    // count here as well means adding a preset without extending the sweep
    // fails one of the two tests rather than silently shipping an unproved
    // ladder.
    assert_eq!(Preset::ALL.len(), 4);
    for preset in Preset::ALL {
        let thresholds = preset.thresholds();
        let mut state = LoopState::with_profile("goal", crate::policy::LoopProfile::of(preset));
        state.blocked = thresholds.blocked;

        let rendered = evaluate_ladder(&state, "loop").expect("the generated ladder evaluates");
        assert_eq!(
            rendered,
            Route::Blocked,
            "{preset} disagreed at the top rung"
        );
    }
}

#[test]
fn assembling_twice_produces_the_same_graph_signature() {
    let one = assembled(Preset::Balanced).expect("assembles");
    let two = assembled(Preset::Balanced).expect("assembles");
    let other = assembled(Preset::Exploratory).expect("assembles");

    assert_eq!(
        one.signature().expect("signed").as_str(),
        two.signature().expect("signed").as_str()
    );
    // A threshold change changes the topology, so it must change the signature:
    // that is what makes an incompatible resume an error rather than silent
    // corruption.
    assert_ne!(
        one.signature().expect("signed").as_str(),
        other.signature().expect("signed").as_str()
    );
}

#[test]
fn an_assembled_loop_carries_its_preset_thresholds_and_budget() {
    let assembled = assembled(Preset::Cautious).expect("assembles");

    assert_eq!(assembled.preset(), Preset::Cautious);
    assert_eq!(assembled.profile().thresholds.unverified, 1);
    assert_eq!(assembled.budget().caps(), Caps::default());
}

#[test]
fn a_report_only_run_emits_a_different_topology_and_signature() {
    let unattended = assembled(Preset::Balanced).expect("assembles");
    let reporting = assembled(Preset::Balanced)
        .expect("assembles")
        .autonomy(Autonomy::Report);

    assert_ne!(
        unattended.signature().expect("signed").as_str(),
        reporting.signature().expect("signed").as_str()
    );
}

// ------------------------------------------------------------- the reflection

fn report_with(outcomes: Vec<(&str, Vec<Artifact>)>) -> Value {
    let report = AttemptReport {
        pass: 0,
        route: "retry".to_owned(),
        directives: Vec::new(),
        artifacts: outcomes
            .iter()
            .flat_map(|(_, artifacts)| artifacts.clone())
            .collect(),
        outcomes: outcomes
            .into_iter()
            .map(|(reply, artifacts)| {
                crate::harness::DelegationOutcome::answered(
                    crate::harness::Brief::new("do it"),
                    reply,
                )
                .with_artifacts(artifacts)
            })
            .collect(),
    };
    serde_json::to_value(report).expect("a report serializes")
}

#[test]
fn a_solved_marker_with_an_artifact_and_consistency_ends_the_loop() {
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let outcome = Reflect
        .evaluate(
            &base,
            &report_with(vec![(
                &format!("{SOLVED_MARKER}: the bound holds"),
                vec![Artifact::new("bound.md", "the proof")],
            )]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert!(outcome.state.solved);
    assert_eq!(outcome.state.banked, 1);
    assert!(Reflect.may_conclude());
}

#[test]
fn a_solved_marker_without_an_artifact_does_not_end_the_loop() {
    // The confabulation case. The marker is the cheap half of the test and the
    // evidence is the expensive half; a claim alone is not a verdict.
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let outcome = Reflect
        .evaluate(
            &base,
            &report_with(vec![(&format!("{SOLVED_MARKER}, trust me"), Vec::new())]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert!(!outcome.state.solved);
    assert_eq!(outcome.state.banked, 0);
    assert_eq!(outcome.state.unverified, 1);
    assert!(
        outcome
            .contribution
            .lesson
            .as_deref()
            .expect("the near miss is worth recording")
            .contains("left nothing behind")
    );
}

#[test]
fn a_marker_from_one_specialist_and_an_artifact_from_another_is_not_consistency() {
    // Internal consistency is the third condition, and it is the one a naive
    // "marker AND artifact" check misses: the specialist that claimed it has to
    // be the one that left something behind.
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let outcome = Reflect
        .evaluate(
            &base,
            &report_with(vec![
                (&format!("{SOLVED_MARKER}, trust me"), Vec::new()),
                (
                    "still working on it",
                    vec![Artifact::new("notes.md", "notes")],
                ),
            ]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert!(!outcome.state.solved);
    assert_eq!(outcome.state.unverified, 1);
}

#[test]
fn an_ordinary_pass_neither_concludes_nor_penalises() {
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let outcome = Reflect
        .evaluate(
            &base,
            &report_with(vec![(
                "progress, no answer",
                vec![Artifact::new("a.md", "a")],
            )]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert!(!outcome.state.solved);
    assert_eq!(outcome.state.unverified, 0);
    assert!(outcome.contribution.lesson.is_none());
}

#[test]
fn a_pass_with_no_artifact_at_all_says_so() {
    let thresholds = Thresholds::default();

    let outcome = Reflect
        .evaluate(
            &LoopState::new("goal"),
            &report_with(vec![("nothing to show", Vec::new())]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert!(
        outcome
            .contribution
            .lesson
            .as_deref()
            .expect("a lesson")
            .contains("no artifact")
    );
}

#[test]
fn an_unreadable_report_never_ends_the_loop() {
    let thresholds = Thresholds::default();

    let outcome = Reflect
        .evaluate(
            &LoopState::new("goal"),
            &json!("not a report at all"),
            observing(&thresholds),
        )
        .expect("the arm survives it");

    assert!(!outcome.state.solved);
    assert!(
        outcome
            .contribution
            .lesson
            .as_deref()
            .expect("a lesson")
            .contains("unreadable")
    );
}

// ------------------------------------------------------------------ the judge

#[test]
fn an_unreadable_verdict_reads_as_the_cheap_outcome() {
    // Reading a serialization slip as a restart throws away a run's work. The
    // asymmetry is the whole rule.
    let thresholds = Thresholds::default();

    let outcome = Judge
        .evaluate(
            &LoopState::new("goal"),
            &json!(null),
            observing(&thresholds),
        )
        .expect("the arm survives it");

    assert_eq!(outcome.contribution.judged, Some(Judgement::Proceed));
    assert_eq!(outcome.contribution.score, Some(0));
    assert!(!Judge.may_conclude());
}

#[test]
fn a_productive_pass_is_told_to_proceed() {
    let thresholds = Thresholds::default();

    let outcome = Judge
        .evaluate(
            &LoopState::new("goal"),
            &report_with(vec![("found something", vec![Artifact::new("a.md", "a")])]),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert_eq!(outcome.contribution.judged, Some(Judgement::Proceed));
    assert_eq!(outcome.contribution.score, Some(2));
    assert!(outcome.contribution.steer.is_none());
}

#[test]
fn an_empty_pass_is_steered_rather_than_restarted_the_first_time() {
    let thresholds = Thresholds::default();
    let report = empty_report();

    let outcome = Judge
        .evaluate(&LoopState::new("goal"), &report, observing(&thresholds))
        .expect("the arm evaluates");

    assert_eq!(outcome.contribution.judged, Some(Judgement::Steer));
    assert!(
        outcome
            .contribution
            .steer
            .as_deref()
            .expect("a correction")
            .contains("narrow the brief")
    );
}

#[test]
fn repeated_empty_passes_become_a_restart() {
    let thresholds = Thresholds::default();
    let mut base = LoopState::new("goal");
    base.unproductive = 3;

    let outcome = Judge
        .evaluate(&base, &empty_report(), observing(&thresholds))
        .expect("the arm evaluates");

    assert_eq!(outcome.contribution.judged, Some(Judgement::Restart));
}

#[test]
fn a_blocked_pass_is_not_steered_about_its_approach() {
    // The machinery did not run, so advice about method is advice about a
    // question the pass never reached.
    let thresholds = Thresholds::default();
    let report = AttemptReport {
        pass: 0,
        route: "retry".to_owned(),
        directives: Vec::new(),
        artifacts: Vec::new(),
        outcomes: vec![crate::harness::DelegationOutcome {
            brief: crate::harness::Brief::new("do it"),
            ending: crate::harness::Ending::Failed,
            artifacts: Vec::new(),
            reply: Some("the sandbox would not start".to_owned()),
        }],
    };

    let outcome = Judge
        .evaluate(
            &LoopState::new("goal"),
            &serde_json::to_value(report).expect("serializes"),
            observing(&thresholds),
        )
        .expect("the arm evaluates");

    assert_eq!(outcome.contribution.judged, Some(Judgement::Proceed));
    assert!(
        outcome
            .contribution
            .steer
            .as_deref()
            .expect("a note")
            .contains("machinery")
    );
}

#[test]
fn the_score_is_capped_and_read_off_the_reports_shape() {
    let report: AttemptReport = serde_json::from_value(report_with(
        (0..20)
            .map(|n| ("answered", vec![Artifact::new(format!("{n}.md"), "a")]))
            .collect(),
    ))
    .expect("a report");

    assert_eq!(Judge::score(&report), 10);
}

#[test]
fn the_two_arms_are_a_legal_set_with_exactly_one_concluder() {
    let set = ArmSet::new(vec![Arc::new(Reflect), Arc::new(Judge)]).expect("a legal set");

    assert_eq!(set.arms().len(), 2);
    assert_eq!(
        set.arms().iter().filter(|arm| arm.may_conclude()).count(),
        1
    );
}

#[test]
fn two_concluding_arms_are_refused() {
    let refused = ArmSet::new(vec![Arc::new(Reflect), Arc::new(SecondConcluder)]);

    assert!(matches!(refused, Err(Error::AmbiguousConclusion { .. })));
}

// ----------------------------------------------------------------- the drive

#[test]
fn an_assembled_loop_runs_end_to_end_over_the_reference_seams() {
    let assembled = solving().expect("assembles");

    let driven = assembled.drive(&quiet()).expect("the loop drives");

    assert_eq!(driven.outcome, Outcome::Success);
    assert!(driven.state.solved);
    assert!(driven.answer().contains("bound the error term"));
    assert_eq!(driven.routes.last(), Some(&Route::Solved));
    assert_eq!(driven.bound, None);
}

#[test]
fn a_loop_that_never_solves_stops_on_a_bound_and_is_never_success() {
    // An error or an exhausted budget is never `Success`. The rule is the
    // reason the classification is adjusted after the bound is known rather
    // than read straight off the last pass.
    let assembled = stalling().expect("assembles");

    let driven = assembled.drive(&quiet()).expect("the loop drives");

    assert_ne!(driven.outcome, Outcome::Success);
    assert_eq!(driven.outcome, Outcome::Exhausted);
    assert!(!driven.state.solved);
}

#[test]
fn every_pass_announces_its_spine_and_every_step_announces_entry_and_exit() {
    // "The run stalled" must be a question the log answers. A live run of this
    // design printed no orchestrator line for 62 minutes and which node was
    // holding could only be inferred from which sub-agents happened to spawn.
    #[derive(Debug, Default)]
    struct Kinds(std::sync::Mutex<Vec<String>>);

    impl Sink for Kinds {
        fn emit(&self, event: &Event) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(event.kind().to_owned());
            }
        }
    }

    let sink = Arc::new(Kinds::default());
    let recorder = Recorder::new("run", sink.clone());

    solving()
        .expect("assembles")
        .drive(&recorder)
        .expect("the loop drives");

    let seen = sink.0.lock().expect("never poisoned").clone();
    for required in [
        "pass_started",
        "pass_finished",
        "step_entered",
        "step_finished",
        "arm_started",
        "arm_finished",
        "merged",
        "judged",
        "routed",
        "loop_finished",
    ] {
        assert!(
            seen.iter().any(|kind| kind == required),
            "missing {required}"
        );
    }
    assert_eq!(
        seen.iter().filter(|kind| *kind == "step_entered").count(),
        seen.iter().filter(|kind| *kind == "step_finished").count()
    );
}

#[test]
fn the_route_a_pass_took_carries_the_counters_it_was_taken_on() {
    let recorder = quiet();

    solving()
        .expect("assembles")
        .drive(&recorder)
        .expect("the loop drives");

    let routed = recorder
        .journal()
        .into_iter()
        .find_map(|entry| match entry.event {
            Event::Routed { reason, .. } => Some(reason),
            _ => None,
        })
        .expect("a route was taken");

    // Re-derivable, not a sentence about it.
    assert!(routed.contains("unproductive="));
    assert!(routed.contains("attempts="));
}

#[test]
fn an_iteration_cap_of_one_stops_after_one_pass() {
    let caps = Caps {
        max_iterations: 1,
        ..Caps::default()
    };
    let assembled = stalling()
        .expect("assembles")
        .with_budget(RunBudget::new(caps).expect("legal caps"));

    let driven = assembled.drive(&quiet()).expect("the loop drives");

    assert_eq!(driven.routes.len(), 1);
    assert_eq!(driven.bound, Some(Bound::Iterations));
    assert_eq!(driven.outcome, Outcome::Exhausted);
}

// ------------------------------------------------------------------ fixtures

/// A second arm that also claims it may conclude, for the refusal test.
#[derive(Debug)]
struct SecondConcluder;

impl Arm for SecondConcluder {
    fn name(&self) -> &'static str {
        "second"
    }

    fn may_conclude(&self) -> bool {
        true
    }

    fn evaluate(
        &self,
        base: &LoopState,
        _report: &Value,
        _ctx: StepContext<'_, NoWrite>,
    ) -> crate::error::Result<crate::arm::ArmOutcome> {
        Ok(crate::arm::ArmOutcome::unchanged("second", base))
    }
}

fn empty_report() -> Value {
    serde_json::to_value(AttemptReport {
        pass: 0,
        route: "retry".to_owned(),
        directives: Vec::new(),
        artifacts: Vec::new(),
        outcomes: vec![crate::harness::DelegationOutcome {
            brief: crate::harness::Brief::new("do it"),
            ending: crate::harness::Ending::TimedOut,
            artifacts: Vec::new(),
            reply: None,
        }],
    })
    .expect("serializes")
}

fn delegates() -> DelegateSet {
    DelegateSet::of(["prover"])
}

fn plan() -> Arc<FixedPlan> {
    Arc::new(FixedPlan::of([(
        "bound",
        "bound the error term",
        "a proved bound",
    )]))
}

fn assembled(preset: Preset) -> crate::error::Result<AssembledLoop> {
    research_loop(
        "bound the error term",
        preset,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                vec![Scripted::Answers {
                    reply: "still working".to_owned(),
                    artifacts: Vec::new(),
                }],
            )],
        )),
    )
}

/// A loop whose specialist solves it, with the artifact to back the claim.
fn solving() -> crate::error::Result<AssembledLoop> {
    research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                vec![Scripted::Answers {
                    reply: format!("{SOLVED_MARKER}: the bound holds"),
                    artifacts: vec![Artifact::new("bound.md", "the proof")],
                }],
            )],
        )),
    )
}

/// A loop whose specialist never comes back with anything.
fn stalling() -> crate::error::Result<AssembledLoop> {
    research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                vec![Scripted::NeverCompletes {
                    artifacts: Vec::new(),
                }],
            )],
        )),
    )
}

// ------------------------------------------------------ the other node bodies

use super::{Advance, ArmStep, Converge, Gather};
use crate::step::{CanWrite, Step};

fn advancing(pass: u32, thresholds: &Thresholds) -> StepContext<'_, CanWrite> {
    StepContext::advancing(pass, thresholds)
}

fn dispatcher(script: Vec<Scripted>) -> Arc<Inline> {
    Arc::new(Inline::of(delegates(), [("prover".to_owned(), script)]))
}

#[test]
fn research_appends_what_it_found_to_the_lessons_and_moves_no_counter() {
    // Research is not an attempt at the goal. Letting it reset `unproductive`
    // would let a run stay out of the diversify branch by looking things up.
    let thresholds = Thresholds::default();
    let gather = Gather::new(
        delegates(),
        dispatcher(vec![Scripted::Answers {
            reply: "the second term is the hard one".to_owned(),
            artifacts: Vec::new(),
        }]),
    );
    let mut before = LoopState::new("goal");
    before.unproductive = 2;

    let after = gather
        .run(before, advancing(0, &thresholds))
        .expect("research runs")
        .into_state();

    assert_eq!(after.lessons.len(), 1);
    assert!(after.lessons[0].contains("from research: the second term"));
    assert_eq!(after.unproductive, 2);
    assert_eq!(after.attempts, 0);
    assert_eq!(after.established, 0);
    assert_eq!(gather.name(), crate::step::STEP_RESEARCH);
}

#[test]
fn research_that_could_not_start_records_nothing_rather_than_a_stack_trace() {
    let thresholds = Thresholds::default();
    let gather = Gather::new(
        delegates(),
        dispatcher(vec![Scripted::Fails {
            reason: "the sandbox would not start".to_owned(),
        }]),
    );

    let after = gather
        .run(LoopState::new("goal"), advancing(0, &thresholds))
        .expect("research survives it")
        .into_state();

    assert!(after.lessons.is_empty());
}

#[test]
fn research_with_no_declared_delegate_is_an_error_rather_than_a_silent_skip() {
    let thresholds = Thresholds::default();
    let gather = Gather::new(DelegateSet::default(), dispatcher(Vec::new()));

    let refused = gather.run(LoopState::new("goal"), advancing(0, &thresholds));

    assert!(matches!(refused, Err(Error::EmptyDelegateSet)));
}

#[test]
fn an_arm_step_runs_its_arm_over_the_one_attempt_report() {
    let thresholds = Thresholds::default();
    let step = ArmStep::new(Arc::new(Reflect));
    let mut state = LoopState::new("goal");
    state.last_attempt = serde_json::to_string(&report_with(vec![(
        &format!("{SOLVED_MARKER}: done"),
        vec![Artifact::new("bound.md", "the proof")],
    )]))
    .expect("a report");

    let after = step
        .run(state, advancing(0, &thresholds))
        .expect("the arm step runs")
        .into_state();

    assert!(after.solved);
    assert_eq!(step.name(), crate::step::STEP_REFLECT);
    assert!(format!("{step:?}").contains("reflect"));
}

#[test]
fn an_arm_step_with_no_report_yet_hands_the_arm_a_null_rather_than_a_stale_one() {
    // Invariant 3, at the step boundary: an arm reads the attempt, and before
    // the first attempt there is nothing to read. Reaching for the accumulator
    // instead would route the first pass on a value nobody produced.
    let thresholds = Thresholds::default();
    let step = ArmStep::new(Arc::new(Judge));

    let after = step
        .run(LoopState::new("goal"), advancing(0, &thresholds))
        .expect("the arm step runs")
        .into_state();

    assert!(!after.solved);
}

#[test]
fn an_arm_step_survives_an_unparseable_report() {
    let thresholds = Thresholds::default();
    let step = ArmStep::new(Arc::new(Reflect));
    let mut state = LoopState::new("goal");
    state.last_attempt = "{ not json".to_owned();

    let after = step
        .run(state, advancing(0, &thresholds))
        .expect("the arm step runs")
        .into_state();

    assert!(!after.solved);
}

#[test]
fn the_pass_step_counts_the_pass_by_assignment_and_consumes_the_steer() {
    // Assignment, not increment: the fold is at-least-once, so a replayed
    // activation after a resume applies the update twice and only an assignment
    // is unchanged by that.
    let thresholds = Thresholds::default();
    let mut state = LoopState::new("goal");
    state.steer = "narrow the claim".to_owned();

    let once = Advance::default()
        .run(state, advancing(4, &thresholds))
        .expect("pass runs")
        .into_state();
    let twice = Advance::default()
        .run(once.clone(), advancing(4, &thresholds))
        .expect("pass runs again")
        .into_state();

    assert_eq!(once.passes, 5);
    assert_eq!(twice, once);
    assert!(once.steer.is_empty());
    assert_eq!(Advance::default().name(), crate::step::STEP_PASS);
}

#[test]
fn the_merge_step_folds_every_arms_counters_by_delta() {
    // A reset from one arm and an increment from another, computed against one
    // shared base, land together instead of overwriting one another.
    let thresholds = Thresholds::default();
    let mut base = LoopState::new("goal");
    base.unproductive = 1;

    let mut productive = base.clone();
    productive.unproductive = 0;
    productive.established = 2;

    let mut restarted = base.clone();
    restarted.unproductive = 2;
    restarted.restarts = 1;

    let merged = merge_of(
        &base,
        &[("reflect", &productive), ("judge", &restarted)],
        &thresholds,
    )
    .expect("the merge folds");

    // 1 + (-1) + (+1). Both movements land, which is the point: a merge that
    // took the last writer would give 2, and one that added the arms' absolute
    // values would give 3. Neither is what the two arms said.
    assert_eq!(merged.unproductive, 1);
    assert_eq!(merged.restarts, 1);
    assert_eq!(merged.established, 2);
}

#[test]
fn the_merge_step_is_order_independent() {
    // The engine states outright that channel update ordering is arbitrary, so
    // a reducer that depends on arrival order is a bug nothing would report.
    let thresholds = Thresholds::default();
    let mut base = LoopState::new("goal");
    base.unproductive = 1;

    let mut one = base.clone();
    one.unproductive = 0;
    one.banked = 3;
    let mut two = base.clone();
    two.unproductive = 2;
    two.established = 5;

    let forwards = merge_of(&base, &[("reflect", &one), ("judge", &two)], &thresholds)
        .expect("the merge folds");
    let backwards = merge_of(&base, &[("judge", &two), ("reflect", &one)], &thresholds)
        .expect("the merge folds");

    assert_eq!(forwards, backwards);
}

#[test]
fn the_merge_step_carries_each_arms_narrative_claim_through() {
    // The round trip that makes the graph path work at all: an arm flattens its
    // contribution into the state it returns, and the merge reads it back out.
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let mut reflection = base.clone();
    Contribution {
        arm: "reflect",
        lesson: Some("the oracle disagreed".to_owned()),
        ..Contribution::new("reflect")
    }
    .apply_to(&mut reflection);

    let mut judgement = base.clone();
    Contribution {
        arm: "judge",
        steer: Some("narrow the claim".to_owned()),
        score: Some(7),
        judged: Some(Judgement::Steer),
        ..Contribution::new("judge")
    }
    .apply_to(&mut judgement);

    let merged = merge_of(
        &base,
        &[("reflect", &reflection), ("judge", &judgement)],
        &thresholds,
    )
    .expect("the merge folds");

    assert_eq!(merged.lessons, vec!["the oracle disagreed".to_owned()]);
    assert_eq!(merged.steer, "narrow the claim");
    assert_eq!(merged.scores, vec![7]);
    assert_eq!(merged.judged, Judgement::Steer);
}

#[test]
fn two_arms_claiming_the_same_narrative_field_is_refused_at_the_merge() {
    // A wiring mistake with no correct resolution. Picking a winner would be
    // arrival-order dependence wearing a merge's clothes.
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");

    let mut one = base.clone();
    one.steer = "go left".to_owned();
    let mut two = base.clone();
    two.steer = "go right".to_owned();

    let refused = merge_of(&base, &[("reflect", &one), ("judge", &two)], &thresholds);

    assert!(matches!(
        refused,
        Err(Error::ContestedField { field: "steer", .. })
    ));
}

#[test]
fn an_arm_missing_from_the_merges_arguments_is_an_error() {
    // Under this engine an expression that failed to resolve yields `null`.
    // Folding what is there and shrugging at the rest would turn a broken
    // binding into a route taken on evidence nobody gathered.
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");
    let candidate = base.clone();

    let refused = merge_of(&base, &[("reflect", &candidate)], &thresholds);

    assert!(matches!(
        refused,
        Err(Error::MalformedStepPayload { field: "arms" })
    ));
}

#[test]
fn an_arm_whose_output_is_null_is_an_error_rather_than_a_smaller_fold() {
    let thresholds = Thresholds::default();
    let base = LoopState::new("goal");
    let arms = ArmSet::new(vec![Arc::new(Reflect), Arc::new(Judge)]).expect("a legal set");
    let args = json!({
        "arms": {
            "reflect": serde_json::to_value(&base).expect("encodes"),
            "judge": Value::Null,
        }
    });

    let refused = Converge::new(arms).run(base, StepContext::advancing_with(0, &thresholds, &args));

    assert!(matches!(
        refused,
        Err(Error::MalformedStepPayload { field: "arms" })
    ));
}

#[test]
fn a_merge_invoked_with_no_arguments_at_all_is_an_error() {
    let thresholds = Thresholds::default();
    let arms = ArmSet::new(vec![Arc::new(Reflect), Arc::new(Judge)]).expect("a legal set");

    let refused = Converge::new(arms).run(LoopState::new("goal"), advancing(0, &thresholds));

    assert!(matches!(
        refused,
        Err(Error::MalformedStepPayload { field: "arms" })
    ));
}

#[test]
fn the_merge_step_answers_to_the_name_the_graph_addresses() {
    let arms = ArmSet::new(vec![Arc::new(Reflect), Arc::new(Judge)]).expect("a legal set");
    let step = Converge::new(arms);

    assert_eq!(step.name(), crate::loops::STEP_MERGE);
    assert!(format!("{step:?}").contains("reflect"));
}

// ---------------------------------------------- the contribution round trip

#[test]
fn applying_a_contribution_and_reading_it_back_is_the_identity() {
    // If these two ever stop being inverses, an arm's contribution silently
    // stops reaching the accumulator, which is exactly the class of failure
    // this crate exists to refuse.
    let base = LoopState::new("goal");
    let claimed = Contribution {
        arm: "reflect",
        lesson: Some("read the error first".to_owned()),
        steer: Some("narrow it".to_owned()),
        score: Some(6),
        judged: Some(Judgement::Restart),
        last_attempt: Some("the report".to_owned()),
        amendment: Some(crate::policy::Amendment::new(
            "reflect",
            0,
            crate::policy::Change::Threshold {
                field: crate::policy::ThresholdField::Stuck,
                to: 3,
            },
            "the round trip has to carry it too",
        )),
    };

    let mut candidate = base.clone();
    claimed.apply_to(&mut candidate);

    assert_eq!(
        Contribution::claimed_from("reflect", &base, &candidate),
        claimed
    );
}

#[test]
fn an_arm_that_touched_nothing_claims_nothing() {
    let base = LoopState::new("goal");

    let read_back = Contribution::claimed_from("reflect", &base, &base.clone());

    assert!(read_back.is_empty());
}

#[test]
fn a_contribution_read_back_onto_a_non_empty_base_claims_only_the_new_entries() {
    let mut base = LoopState::new("goal");
    base.lessons = vec!["an old lesson".to_owned()];
    base.scores = vec![3];
    base.steer = "an old steer".to_owned();

    let mut candidate = base.clone();
    Contribution {
        arm: "judge",
        lesson: Some("a new lesson".to_owned()),
        score: Some(9),
        ..Contribution::new("judge")
    }
    .apply_to(&mut candidate);

    let read_back = Contribution::claimed_from("judge", &base, &candidate);

    assert_eq!(read_back.lesson.as_deref(), Some("a new lesson"));
    assert_eq!(read_back.score, Some(9));
    // Unchanged, so unclaimed: the arm did not write it and must not be
    // recorded as having done so, or a second arm writing it would be refused
    // for a collision that never happened.
    assert_eq!(read_back.steer, None);
}

// ------------------------------------------------------------------ fixtures

/// Runs the merge step over `arms`, exactly as the emitted node would.
fn merge_of(
    base: &LoopState,
    arms: &[(&str, &LoopState)],
    thresholds: &Thresholds,
) -> crate::error::Result<LoopState> {
    let mut returned = serde_json::Map::new();
    for (name, state) in arms {
        returned.insert(
            (*name).to_owned(),
            serde_json::to_value(state).expect("a state encodes"),
        );
    }
    let args = json!({ "arms": Value::Object(returned) });
    let set = ArmSet::new(vec![Arc::new(Reflect), Arc::new(Judge)]).expect("a legal set");

    Converge::new(set)
        .run(
            base.clone(),
            StepContext::advancing_with(base.passes, thresholds, &args),
        )
        .map(crate::step::Advanced::into_state)
}

#[test]
fn every_preset_can_reach_its_attempt_ceiling() {
    // The loop head's `max_iterations` is a runaway backstop, and a backstop
    // below a preset's own attempt ceiling truncates the run before its
    // thresholds ever fire. `Preset::Persistent` asks for twelve attempts; a
    // default cap of eight would stop it four short, and the run would report
    // "not solved within N attempts" having never spent N.
    let cap = crate::budget::Caps::default().max_iterations;
    for preset in Preset::ALL {
        let ceiling = preset.thresholds().max_attempts;
        assert!(
            ceiling <= cap,
            "{preset} asks for {ceiling} attempts under a cap of {cap}",
        );
    }
}

#[test]
fn a_presets_wire_name_is_the_name_it_renders() {
    // A preset travels inside the accumulator as part of the profile, so its
    // serde names are a wire format. Two spellings of one variant would decode
    // as an error somewhere nobody is looking.
    for preset in Preset::ALL {
        assert_eq!(
            serde_json::to_value(preset).unwrap(),
            serde_json::Value::String(preset.as_str().to_owned()),
        );
        assert_eq!(
            serde_json::from_value::<Preset>(serde_json::json!(preset.as_str())).unwrap(),
            preset,
        );
    }
    assert_eq!(Preset::default(), Preset::Balanced);
}

// --- adaptation ------------------------------------------------------------

/// A state under `preset` with `edit` applied.
fn tuned_state(preset: Preset, edit: impl FnOnce(&mut LoopState)) -> LoopState {
    let mut state = LoopState::with_profile("goal", crate::policy::LoopProfile::of(preset));
    edit(&mut state);
    state
}

/// What the rule tuner proposes for `state`, if anything.
fn proposal(state: &LoopState) -> Option<crate::policy::Change> {
    use crate::arm::Tuner as _;

    let thresholds = state.profile.thresholds;
    Rules
        .propose(
            state,
            &serde_json::Value::Null,
            crate::step::StepContext::observing(state.passes, &thresholds),
        )
        .expect("the rule tuner is total")
        .map(|amendment| amendment.change)
}

#[test]
fn the_rule_tuner_says_nothing_about_an_ordinary_pass() {
    // The ordinary answer. A tuner that proposes on every pass has mistaken its
    // own budget for a target.
    assert_eq!(proposal(&tuned_state(Preset::Balanced, |_| {})), None);
    assert_eq!(
        proposal(&tuned_state(Preset::Balanced, |state| {
            state.unproductive = 1;
            state.scores = vec![4, 7];
        })),
        None
    );
}

#[test]
fn the_rule_tuner_raises_patience_only_once_diversifying_has_failed() {
    // `unproductive` equal to `stuck` is the pass that diversifies; nothing has
    // been learned yet about whether the variation paid. One past it is the
    // pass after a diversify that was still unproductive, which is the run's
    // own evidence that the threshold was too low for this domain.
    let at_the_bound = tuned_state(Preset::Balanced, |state| state.unproductive = 2);
    assert_eq!(proposal(&at_the_bound), None);

    let past_it = tuned_state(Preset::Balanced, |state| state.unproductive = 3);
    assert_eq!(
        proposal(&past_it),
        Some(crate::policy::Change::Threshold {
            field: crate::policy::ThresholdField::Stuck,
            to: 3,
        })
    );
}

#[test]
fn the_rule_tuner_asks_for_a_threshold_once_and_not_again() {
    // Asking twice is how a tuner spends its whole amendment budget arriving
    // where one proposal would have put it. A refusal counts as asked: a bound
    // that said no once will say no again.
    let bounds = Preset::Balanced.bounds();
    let mut state = tuned_state(Preset::Balanced, |state| state.unproductive = 3);

    let first = proposal(&state).expect("the first pass past the bound proposes");
    state.profile.fold(
        crate::policy::Amendment::new("tune", 1, first, "because"),
        &bounds,
    );

    state.unproductive = 9;
    assert_eq!(proposal(&state), None);
}

#[test]
fn the_rule_tuner_spends_less_when_the_machinery_is_failing() {
    // Infrastructure first, and the only move available that costs less rather
    // than more. A run whose sandbox will not start has learned nothing about
    // its own patience.
    let blocked = tuned_state(Preset::Balanced, |state| {
        state.blocked = 1;
        state.unproductive = 9;
    });

    assert_eq!(
        proposal(&blocked),
        Some(crate::policy::Change::Cap {
            field: crate::policy::CapField::MaxModelCalls,
            to: u64::from(crate::budget::Caps::default().max_model_calls / 2),
        }),
        "the blocked rule outranks the patience rule",
    );
}

#[test]
fn the_rule_tuner_mutes_a_judge_that_has_stopped_discriminating() {
    // Silence, not "scored worse". The loop has no per-arm reward to rank arms
    // by, so the rule fires on a signal that has stopped varying and never on a
    // comparison it cannot make.
    let flat = tuned_state(Preset::Balanced, |state| {
        state.scores = vec![9, 4, 4, 4];
    });
    assert_eq!(
        proposal(&flat),
        Some(crate::policy::Change::MuteArm {
            arm: crate::step::STEP_JUDGE.to_owned(),
        })
    );

    let varying = tuned_state(Preset::Balanced, |state| {
        state.scores = vec![4, 4, 5];
    });
    assert_eq!(proposal(&varying), None);
}

#[test]
fn the_rule_tuner_proposes_on_exactly_these_passes() {
    // The whole behavior as one sequence, so a change to any rule shows here as
    // a changed list rather than as one test flipping.
    let sequence: Vec<Option<&str>> = [
        (0_u32, 0_u32, vec![]),
        (1, 0, vec![4]),
        (2, 0, vec![4, 4]),
        (3, 0, vec![4, 4, 4]),
        (0, 1, vec![]),
    ]
    .into_iter()
    .map(|(unproductive, blocked, scores)| {
        let state = tuned_state(Preset::Balanced, |state| {
            state.unproductive = unproductive;
            state.blocked = blocked;
            state.scores = scores;
        });
        proposal(&state).map(|change| match change {
            crate::policy::Change::Threshold { .. } => "stuck",
            crate::policy::Change::Cap { .. } => "cap",
            crate::policy::Change::MuteArm { .. } => "mute",
            crate::policy::Change::UnmuteArm { .. } => "unmute",
        })
    })
    .collect();

    assert_eq!(sequence, vec![None, None, None, Some("stuck"), Some("cap")],);
}

#[test]
fn an_amendment_does_not_change_the_route_of_the_pass_that_proposed_it() {
    // The fold lands at `pass`, the loop's single exit, so a proposal reaches
    // the router exactly one pass later. An arm that could change a threshold
    // and have the same pass's route read it would make the route depend on
    // whether the tuner finished before the routing node.
    let mut state = tuned_state(Preset::Balanced, |state| state.unproductive = 2);
    let proposed = crate::policy::Amendment::new(
        "tune",
        0,
        crate::policy::Change::Threshold {
            field: crate::policy::ThresholdField::Stuck,
            to: 4,
        },
        "because",
    );
    state.profile.thresholds.stuck = 2;

    let mut carrying = state.clone();
    crate::state::Contribution {
        amendment: Some(proposed),
        ..crate::state::Contribution::new("tune")
    }
    .apply_to(&mut carrying);

    // Proposed, and the route is the one the *current* thresholds give.
    assert_eq!(crate::policy::route(&carrying), Route::Diversify);

    // Folded at `pass`, and the next route is the one the new thresholds give.
    let advanced = Advance::new(Preset::Balanced.bounds())
        .run(
            carrying,
            crate::step::StepContext::advancing(0, &Thresholds::default()),
        )
        .expect("the pass step folds")
        .into_state();

    assert_eq!(advanced.profile.thresholds.stuck, 4);
    assert_eq!(advanced.profile.revision, 1);
    assert!(advanced.proposed().is_none(), "a proposal lands once");
    assert_eq!(crate::policy::route(&advanced), Route::Retry);
}

#[test]
fn a_muted_arm_still_runs_its_node_and_still_converges() {
    // Muting removes the arm's work, never its edges. Dropping a convergence
    // edge would leave the barrier waiting on an arm nothing will activate.
    let mut state = tuned_state(Preset::Balanced, |state| {
        state.last_attempt = String::new();
    });
    state
        .profile
        .muted
        .insert(crate::step::STEP_JUDGE.to_owned());

    let step = ArmStep::new(std::sync::Arc::new(Judge));
    let returned = step
        .run(
            state.clone(),
            crate::step::StepContext::advancing(0, &Thresholds::default()),
        )
        .expect("a muted arm still returns")
        .into_state();

    // A zero delta and an empty contribution: it ran, and it contributed
    // nothing, which is what the merge needs from it.
    assert_eq!(returned, state);
    assert_eq!(returned.delta_from(&state), crate::Delta::default());
    assert!(
        crate::Contribution::claimed_from(crate::step::STEP_JUDGE, &state, &returned).is_empty()
    );
}

#[test]
fn a_tuned_run_carries_a_third_arm_and_a_different_graph() {
    let plain = assembled(Preset::Balanced).expect("the preset assembles");
    let tuned = tuned_research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                vec![Scripted::Answers {
                    reply: "still working".to_owned(),
                    artifacts: Vec::new(),
                }],
            )],
        )),
    )
    .expect("the tuned preset assembles");

    // A run that can revise itself and one that cannot are different loops, and
    // the difference is in the topology rather than in an argument.
    let graph = tuned.graph().expect("the tuned graph validates");
    assert!(graph.node(Rules::NAME).is_some());
    assert!(
        plain
            .graph()
            .expect("the plain graph validates")
            .node(Rules::NAME)
            .is_none()
    );
    assert_ne!(
        plain.signature().expect("signed"),
        tuned.signature().expect("signed"),
    );
}

#[test]
fn a_tuned_run_reports_every_revision_and_every_refusal() {
    // End to end, over the reference seams: the specialist keeps failing, the
    // run notices the machinery rather than the work, and its report says what
    // it changed about itself.
    let tuned = tuned_research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                // Every pass fails on the machinery rather than on the work,
                // which is the run the blocked rule exists for.
                std::iter::repeat_with(|| Scripted::Fails {
                    reason: "the sandbox would not start".to_owned(),
                })
                .take(12)
                .collect(),
            )],
        )),
    )
    .expect("the tuned preset assembles");

    let sink = Arc::new(LineSink::new(std::io::sink()));
    let recorder = Recorder::new("run", sink);
    let driven = tuned.drive(&recorder).expect("a tuned run drives");

    // The profile is an output, not a detail of the state.
    assert_eq!(driven.profile, driven.state.profile);

    // Whatever it proposed, every proposal is in the record and in the report,
    // and the events say which were folded and which were refused.
    let amended = recorder
        .journal()
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.event,
                Event::Amended { .. } | Event::AmendmentRefused { .. }
            )
        })
        .count();
    assert_eq!(amended, driven.profile.history.len());

    if !driven.profile.history.is_empty() {
        assert!(driven.answer().contains("Revised itself"));
        for recorded in &driven.profile.history {
            assert!(
                driven
                    .answer()
                    .contains(&recorded.amendment.change.to_string()),
                "the report omits {recorded}",
            );
        }
    }
}

#[test]
fn a_plain_run_proposes_nothing_and_says_nothing_about_revisions() {
    // The tuner is optional, and a loop without one costs nothing for it and
    // reports no section about it.
    let driven = assembled(Preset::Balanced)
        .expect("the preset assembles")
        .drive(&Recorder::new(
            "run",
            Arc::new(LineSink::new(std::io::sink())),
        ))
        .expect("a plain run drives");

    assert!(driven.profile.history.is_empty());
    assert_eq!(driven.profile.revision, 0);
    assert!(!driven.answer().contains("Revised itself"));
}

#[test]
fn a_run_the_machinery_keeps_failing_spends_less_rather_than_more() {
    // The end-to-end version of the blocked rule, asserted rather than left to
    // the "if it proposed anything" branch above: a run whose specialist never
    // starts should reach for a smaller model-call allowance, not more
    // attempts.
    let tuned = tuned_research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            [(
                "prover".to_owned(),
                std::iter::repeat_with(|| Scripted::Fails {
                    reason: "the sandbox would not start".to_owned(),
                })
                .take(12)
                .collect(),
            )],
        )),
    )
    .expect("the tuned preset assembles");

    let driven = tuned
        .drive(&Recorder::new(
            "run",
            Arc::new(LineSink::new(std::io::sink())),
        ))
        .expect("a tuned run drives");

    assert!(
        driven.profile.history.iter().any(|recorded| matches!(
            recorded.amendment.change,
            crate::policy::Change::Cap {
                field: crate::policy::CapField::MaxModelCalls,
                ..
            }
        )),
        "a blocked run proposed nothing about its spend: {:?}",
        driven.profile.history,
    );
    assert!(driven.profile.applied() > 0);
    assert!(driven.profile.caps.max_model_calls < crate::budget::Caps::default().max_model_calls);
}
