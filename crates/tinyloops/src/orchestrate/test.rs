//! Unit tests for the orchestrator: the board's id stability and counts, the
//! registration-time refusals, and the three steps' bindings.
//!
//! Every test here asserts something the specification calls a *property of the
//! registration* rather than of a prompt, so each one is written against the
//! constructor or the emitted state rather than against a rendered brief.

use std::sync::Arc;

use super::{
    Attempt, AttemptReport, Compose, DelegateSet, FixedPlan, Inline, Orchestrator, Plan, Report,
    Specialists, Summarize, Task, TaskBoard, TaskId, TaskStatus,
};
use crate::error::Error;
use crate::harness::{
    Artifact, Brief, DEFAULT_MAILBOX_CAPACITY, Mailbox, Note, RoleGrant, RoleRegistry, Scripted,
    SinkDrops, Tier,
};
use crate::observe::{Event, Sink};
use crate::policy::{Route, Thresholds};
use crate::state::LoopState;
use crate::step::{CanWrite, Step, StepContext};
use crate::tools::{ToolGrant, ToolGroup};

fn id(name: &str) -> TaskId {
    TaskId::new(name).expect("a non-empty id")
}

fn context(pass: u32, thresholds: &Thresholds) -> StepContext<'_, CanWrite> {
    StepContext::advancing(pass, thresholds)
}

// ---------------------------------------------------------------- the board

#[test]
fn a_task_carries_an_id_a_statement_a_criterion_a_status_and_a_pass() {
    let task = Task::new(id("bound"), "bound the error", "a proved bound", 3);

    assert_eq!(task.id.as_str(), "bound");
    assert_eq!(task.statement, "bound the error");
    assert_eq!(task.criterion, "a proved bound");
    assert_eq!(task.status, TaskStatus::Open);
    assert_eq!(task.touched, 3);
}

#[test]
fn an_empty_task_id_is_refused() {
    assert!(matches!(TaskId::new("  "), Err(Error::EmptyName)));
}

#[test]
fn reusing_an_id_for_a_different_task_is_an_error() {
    let mut board = TaskBoard::new();
    board
        .add(Task::new(id("one"), "first", "criterion", 0))
        .expect("the first add succeeds");

    let clash = board.add(Task::new(id("one"), "something else", "criterion", 1));

    assert!(matches!(clash, Err(Error::DuplicateTask { ref id }) if id == "one"));
    // And the original survives rather than being half-replaced.
    assert_eq!(board.find(&id("one")).expect("still there").statement, "first");
}

#[test]
fn restating_keeps_the_id_and_moves_the_pass() {
    let mut board = TaskBoard::new();
    board
        .add(Task::new(id("one"), "first", "criterion", 0))
        .expect("the add succeeds");

    board
        .restate(&id("one"), "first, more precisely", "a tighter criterion", 4)
        .expect("the restatement succeeds");

    let task = board.find(&id("one")).expect("still there");
    assert_eq!(task.statement, "first, more precisely");
    assert_eq!(task.criterion, "a tighter criterion");
    assert_eq!(task.touched, 4);
    assert_eq!(board.len(), 1);
}

#[test]
fn restating_or_settling_an_absent_task_names_the_id() {
    let mut board = TaskBoard::new();

    let restated = board.restate(&id("ghost"), "x", "y", 0);
    let settled = board.settle(&id("ghost"), TaskStatus::Discharged, 0);

    assert!(matches!(restated, Err(Error::UnknownTask { ref id }) if id == "ghost"));
    assert!(matches!(settled, Err(Error::UnknownTask { ref id }) if id == "ghost"));
}

#[test]
fn counts_are_readable_without_parsing_prose() {
    let mut board = TaskBoard::new();
    for name in ["a", "b", "c", "d", "e"] {
        board
            .add(Task::new(id(name), name, "criterion", 0))
            .expect("distinct ids");
    }
    for name in ["a", "b", "c"] {
        board
            .settle(&id(name), TaskStatus::Discharged, 1)
            .expect("the task exists");
    }

    assert_eq!(board.count(TaskStatus::Discharged), 3);
    assert_eq!(board.count(TaskStatus::Open), 2);
    assert_eq!(board.len(), 5);
    assert_eq!(board.outstanding().len(), 2);
    assert!(!board.is_complete());
}

#[test]
fn an_empty_board_is_not_complete() {
    // Nothing stated is not everything done, and reporting it as done would let
    // a failed plan read as success.
    assert!(!TaskBoard::new().is_complete());
    assert!(TaskBoard::new().is_empty());
    assert_eq!(TaskBoard::new().planned_at(), None);
}

#[test]
fn a_board_whose_every_task_is_settled_is_complete() {
    let mut board = TaskBoard::new();
    board.add(Task::new(id("a"), "a", "c", 0)).expect("added");
    board.add(Task::new(id("b"), "b", "c", 0)).expect("added");
    board
        .settle(&id("a"), TaskStatus::Discharged, 1)
        .expect("exists");
    board
        .settle(&id("b"), TaskStatus::Abandoned, 1)
        .expect("exists");

    assert!(board.is_complete());
    assert!(TaskStatus::Abandoned.is_settled());
    assert!(!TaskStatus::InFlight.is_settled());
}

#[test]
fn every_status_has_a_hand_written_wire_name() {
    assert_eq!(TaskStatus::Open.as_str(), "open");
    assert_eq!(TaskStatus::InFlight.as_str(), "in_flight");
    assert_eq!(TaskStatus::Discharged.as_str(), "discharged");
    assert_eq!(TaskStatus::Abandoned.as_str(), "abandoned");
    assert_eq!(TaskStatus::default(), TaskStatus::Open);
    assert_eq!(id("shown").to_string(), "shown");
}

#[test]
fn the_board_round_trips_through_the_accumulator() {
    // The checkpoint-and-resume test. The board rides in `LoopState`, so what
    // survives a serialization round trip is what survives a resume.
    let mut state = LoopState::new("goal");
    state
        .board
        .add(Task::new(id("one"), "first", "criterion", 0))
        .expect("added");
    state
        .board
        .add(Task::new(id("two"), "second", "criterion", 0))
        .expect("added");
    state
        .board
        .settle(&id("two"), TaskStatus::Discharged, 2)
        .expect("exists");
    state.board.planned(2);

    let encoded = serde_json::to_string(&state).expect("the accumulator serializes");
    let restored: LoopState = serde_json::from_str(&encoded).expect("and deserializes");

    assert_eq!(restored.board, state.board);
    assert_eq!(restored.board.planned_at(), Some(2));
    assert_eq!(
        restored
            .board
            .find(&id("two"))
            .expect("the id survived")
            .status,
        TaskStatus::Discharged
    );
}

#[test]
fn the_fold_carries_the_board_and_the_answer_through_untouched() {
    // Sole authorship, structurally: no delta and no contribution has a slot
    // that reaches either field, so no arm can write one however it is wired.
    let mut base = LoopState::new("goal");
    base.board.add(Task::new(id("one"), "first", "c", 0)).expect("added");
    base.answer = "the run's account".to_owned();

    let mut arm = base.clone();
    arm.established = 3;
    arm.board = TaskBoard::new();
    arm.answer = "an arm's account".to_owned();

    let folded = base.apply(&[arm.delta_from(&base)]);

    assert_eq!(folded.established, 3);
    assert_eq!(folded.board, base.board);
    assert_eq!(folded.answer, "the run's account");
}

// -------------------------------------------------------- the registration

fn read_only() -> Result<Orchestrator, Error> {
    Orchestrator::new(ToolGrant::read_only(), DelegateSet::of(["prover", "refuter"]))
}

#[test]
fn a_file_write_tool_in_the_orchestrators_set_fails_construction() {
    let refused = Orchestrator::new(
        ToolGrant::of(&[ToolGroup::Read, ToolGroup::Edit]),
        DelegateSet::of(["prover"]),
    );

    let Err(error) = refused else {
        panic!("an editing grant must be refused");
    };
    assert!(matches!(
        error,
        Error::ExecutionToolInOrchestrator { group: "edit" }
    ));
    assert!(error.to_string().contains("edit"));
}

#[test]
fn a_code_runner_in_the_orchestrators_set_fails_construction() {
    let refused = Orchestrator::new(
        ToolGrant::of(&[ToolGroup::Execute]),
        DelegateSet::of(["prover"]),
    );

    let Err(error) = refused else {
        panic!("an executing grant must be refused");
    };
    assert!(matches!(
        error,
        Error::ExecutionToolInOrchestrator { group: "execute" }
    ));
    assert!(error.to_string().contains("execute"));
}

#[test]
fn a_shell_tool_arrives_as_the_execute_group_and_is_refused_first_by_edit() {
    // `ToolGrant::all()` holds both forbidden groups. The check is ordered, so
    // the message is deterministic rather than dependent on set iteration.
    let refused = Orchestrator::new(ToolGrant::all(), DelegateSet::of(["prover"]));

    assert!(matches!(
        refused,
        Err(Error::ExecutionToolInOrchestrator { group: "edit" })
    ));
}

#[test]
fn reading_and_searching_are_left_in_the_grant() {
    let orchestrator = read_only().expect("a read-only grant is legal");

    assert!(orchestrator.grant().holds(ToolGroup::Read));
    assert!(!orchestrator.grant().holds(ToolGroup::Execute));
    assert_eq!(orchestrator.delegates().len(), 2);
    assert!(!orchestrator.delegates().is_empty());
}

#[test]
fn an_orchestrator_with_no_delegates_is_refused() {
    let refused = Orchestrator::new(ToolGrant::read_only(), DelegateSet::default());

    assert!(matches!(refused, Err(Error::EmptyDelegateSet)));
}

#[test]
fn spawning_a_delegate_outside_the_declared_set_is_an_error() {
    let dispatcher = Inline::new(
        DelegateSet::of(["prover"]),
        [("prover".to_owned(), vec![answers("proved")])],
    );

    let refused = dispatcher.dispatch(vec![("intruder".to_owned(), Brief::new("do a thing"))]);

    assert!(matches!(refused, Err(Error::UndeclaredDelegate { ref name }) if name == "intruder"));
}

#[test]
fn it_does_not_fall_back_to_the_host_registry() {
    // The host knows the role. The orchestrator was not told about it. The
    // spawn still fails, because a specialist reachable by accident is one
    // nobody chose.
    let mut registry = RoleRegistry::new();
    declare(&mut registry, "prover");
    declare(&mut registry, "wildcard");
    let orchestrator = Orchestrator::new(ToolGrant::read_only(), DelegateSet::of(["prover"]))
        .expect("a legal registration");
    let delegate = crate::harness::ScriptedDelegate::new(registry, tinyflows::caps::mock::mock_capabilities())
        .scripting("wildcard", vec![answers("I was reachable")]);

    let refused = orchestrator.spawn(&delegate, "wildcard", Brief::new("do a thing"));

    assert!(matches!(refused, Err(Error::UndeclaredDelegate { ref name }) if name == "wildcard"));
    assert!(!orchestrator.may_spawn("wildcard"));
}

#[test]
fn a_declared_delegate_the_registry_lacks_fails_at_wiring_time() {
    let mut registry = RoleRegistry::new();
    declare(&mut registry, "prover");
    let orchestrator = Orchestrator::new(
        ToolGrant::read_only(),
        DelegateSet::of(["prover", "never-declared"]),
    )
    .expect("a legal registration");

    let checked = orchestrator.verify_declared_in(&registry);

    assert!(matches!(checked, Err(Error::UnknownRole { ref role }) if role == "never-declared"));
}

#[test]
fn a_delegate_set_matching_the_registry_verifies() {
    let mut registry = RoleRegistry::new();
    declare(&mut registry, "prover");
    declare(&mut registry, "refuter");
    let orchestrator = read_only().expect("a legal registration");

    assert!(orchestrator.verify_declared_in(&registry).is_ok());
    assert_eq!(
        orchestrator.delegates().names().collect::<Vec<_>>(),
        ["prover", "refuter"]
    );
}

#[test]
fn a_declared_delegate_reaches_the_harness() {
    let mut registry = RoleRegistry::new();
    declare(&mut registry, "prover");
    let orchestrator = Orchestrator::new(ToolGrant::read_only(), DelegateSet::of(["prover"]))
        .expect("a legal registration");
    let delegate = crate::harness::ScriptedDelegate::new(registry, tinyflows::caps::mock::mock_capabilities())
        .scripting("prover", vec![answers("proved")]);

    let ticket = orchestrator
        .spawn(&delegate, "prover", Brief::new("prove it"))
        .expect("a declared delegate spawns");

    assert!(!ticket.id().is_empty());
}

// ------------------------------------------------------------- the cadence

#[test]
fn plan_runs_at_pass_zero_and_then_only_on_its_cadence() {
    let thresholds = Thresholds::default();
    let plan = Plan::new(Arc::new(FixedPlan::of([("one", "first", "criterion")])));

    let ran: Vec<u32> = (0..10)
        .filter(|pass| {
            let before = LoopState::new("goal");
            let after = plan
                .run(before.clone(), context(*pass, &thresholds))
                .expect("the plan step runs")
                .into_state();
            after.board.planned_at().is_some()
        })
        .collect();

    // `plan_interval` is 3 by default, and pass 0 always plans.
    assert_eq!(thresholds.plan_interval, 3);
    assert_eq!(ran, vec![0, 3, 6, 9]);
}

#[test]
fn an_off_cadence_pass_leaves_the_board_exactly_as_it_found_it() {
    let thresholds = Thresholds::default();
    let plan = Plan::new(Arc::new(FixedPlan::of([("one", "first", "criterion")])));
    let mut before = LoopState::new("goal");
    before
        .board
        .add(Task::new(id("existing"), "already stated", "criterion", 0))
        .expect("added");
    before.board.planned(0);

    let after = plan
        .run(before.clone(), context(1, &thresholds))
        .expect("the plan step runs")
        .into_state();

    assert_eq!(after.board, before.board);
}

#[test]
fn re_planning_restates_rather_than_duplicating_an_id() {
    // The stability rule, exercised through the step rather than the board: a
    // decomposer that proposes the same id twice must not grow the board.
    let thresholds = Thresholds::default();
    let plan = Plan::new(Arc::new(FixedPlan::of([("one", "first", "criterion")])));
    let state = plan
        .run(LoopState::new("goal"), context(0, &thresholds))
        .expect("planned")
        .into_state();

    let replanned = plan
        .run(state, context(3, &thresholds))
        .expect("re-planned")
        .into_state();

    assert_eq!(replanned.board.len(), 1);
    assert_eq!(replanned.board.planned_at(), Some(3));
    assert_eq!(
        replanned
            .board
            .find(&id("one"))
            .expect("still there")
            .touched,
        3
    );
}

#[test]
fn a_decomposer_naming_an_empty_id_fails_the_pass() {
    let thresholds = Thresholds::default();
    let plan = Plan::new(Arc::new(FixedPlan::of([("", "first", "criterion")])));

    let failed = plan.run(LoopState::new("goal"), context(0, &thresholds));

    assert!(matches!(failed, Err(Error::EmptyName)));
}

// ------------------------------------------------------------- the attempt

fn answers(reply: &str) -> Scripted {
    Scripted::Answers {
        reply: reply.to_owned(),
        artifacts: Vec::new(),
    }
}

fn declare(registry: &mut RoleRegistry, name: &str) {
    registry
        .declare(
            name,
            "do the thing",
            RoleGrant::none(),
            Some(crate::budget::Caps::default()),
            Tier::Standard,
        )
        .expect("a fresh name");
}

fn attempt_over(script: Vec<(String, Vec<Scripted>)>, delegates: &[&str]) -> Attempt {
    let set = DelegateSet::of(delegates.iter().copied());
    let orchestrator = Orchestrator::new(ToolGrant::read_only(), set.clone())
        .expect("a legal registration");
    Attempt::new(
        orchestrator,
        Arc::new(Inline::new(set, script)),
        Arc::new(Mailbox::new(DEFAULT_MAILBOX_CAPACITY)),
    )
}

fn planned(goal: &str, tasks: &[(&str, &str)]) -> LoopState {
    let mut state = LoopState::new(goal);
    for (id_, statement) in tasks {
        state
            .board
            .add(Task::new(id(id_), *statement, "criterion", 0))
            .expect("distinct ids");
    }
    state
}

#[test]
fn attempt_writes_exactly_one_report_per_pass_at_a_known_address() {
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("proved")])],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    // One address: `last_attempt`, which is what the graph's `attempt` node
    // publishes and what every arm is wired to read.
    let report: AttemptReport =
        serde_json::from_str(&state.last_attempt).expect("the report is a value, not prose");
    assert_eq!(report.pass, 0);
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(state.attempts, 1);
}

#[test]
fn a_briefed_task_moves_to_in_flight_and_no_further() {
    // The attempt never discharges a task on a specialist's say-so; the
    // criterion is checked against the workspace, not against a reply.
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("done, honestly")])],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    assert_eq!(
        state.board.find(&id("one")).expect("still there").status,
        TaskStatus::InFlight
    );
    assert_eq!(state.board.count(TaskStatus::Discharged), 0);
}

#[test]
fn established_is_counted_off_artifacts_rather_than_off_a_reply() {
    let attempt = attempt_over(
        vec![(
            "prover".to_owned(),
            vec![Scripted::Answers {
                reply: "I established a great deal".to_owned(),
                artifacts: vec![Artifact::new("bound.md", "the proved bound")],
            }],
        )],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    assert_eq!(state.established, 1);
}

#[test]
fn a_timed_out_specialist_yields_a_readable_outcome_and_a_report() {
    // A timeout that left an artifact is evidence. The pass still writes a
    // report and does not increment `unproductive` on the strength of the
    // timeout alone.
    let attempt = attempt_over(
        vec![(
            "prover".to_owned(),
            vec![Scripted::NeverCompletes {
                artifacts: vec![Artifact::new("partial.md", "as far as it got")],
            }],
        )],
        &["prover"],
    );
    let thresholds = Thresholds::default();
    let mut before = planned("goal", &[("one", "first")]);
    before.unproductive = 1;

    let state = attempt
        .run(before, context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    let report: AttemptReport = serde_json::from_str(&state.last_attempt).expect("a report");
    assert_eq!(report.outcomes[0].ending.as_str(), "timed_out");
    assert_eq!(report.outcomes[0].brief.task, "first");
    assert!(report.is_informative());
    assert_eq!(state.unproductive, 0);
}

#[test]
fn a_killed_specialist_that_wrote_an_artifact_yields_a_salvaged_attempt() {
    let attempt = attempt_over(
        vec![(
            "prover".to_owned(),
            vec![Scripted::Capped {
                artifacts: vec![Artifact::new("notes.md", "the partial survey")],
            }],
        )],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    let report: AttemptReport = serde_json::from_str(&state.last_attempt).expect("a report");
    assert_eq!(report.outcomes[0].ending.as_str(), "capped");
    assert!(report.outcomes[0].reply.is_none());
    // The artifact is cited, which is the whole point of salvage.
    assert_eq!(report.artifacts, vec![Artifact::new("notes.md", "the partial survey")]);
    assert_eq!(state.established, 1);
    assert_eq!(state.unproductive, 0);
}

#[test]
fn a_pass_that_produced_nothing_at_all_counts_as_unproductive() {
    let attempt = attempt_over(
        vec![(
            "prover".to_owned(),
            vec![Scripted::NeverCompletes {
                artifacts: Vec::new(),
            }],
        )],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    assert_eq!(state.unproductive, 1);
    assert_eq!(state.blocked, 0);
}

#[test]
fn a_specialist_that_never_started_counts_as_blocked_rather_than_unproductive() {
    // Infrastructure failure is not evidence about the goal, and the loop
    // counts it apart from an attempt that ran and did not succeed.
    let attempt = attempt_over(
        vec![(
            "prover".to_owned(),
            vec![Scripted::Fails {
                reason: String::new(),
            }],
        )],
        &["prover"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    let report: AttemptReport = serde_json::from_str(&state.last_attempt).expect("a report");
    assert_eq!(report.blocked(), 1);
    assert_eq!(state.blocked, 1);
    assert_eq!(state.unproductive, 0);
}

#[test]
fn one_answering_specialist_rescues_a_pass_another_timed_out_on() {
    let attempt = attempt_over(
        vec![
            (
                "prover".to_owned(),
                vec![Scripted::NeverCompletes {
                    artifacts: Vec::new(),
                }],
            ),
            ("refuter".to_owned(), vec![answers("a counterexample")]),
        ],
        &["prover", "refuter"],
    );
    let thresholds = Thresholds::default();

    let state = attempt
        .run(
            planned("goal", &[("one", "first"), ("two", "second")]),
            context(0, &thresholds),
        )
        .expect("the attempt runs")
        .into_state();

    let report: AttemptReport = serde_json::from_str(&state.last_attempt).expect("a report");
    assert_eq!(report.outcomes.len(), 2);
    assert!(report.is_informative());
    assert_eq!(state.unproductive, 0);
}

#[test]
fn a_diversify_route_opens_every_delegate_on_one_task() {
    // Sequential revision conditions every next attempt on the same failed
    // framing; drawing several independent attempts does not. The rung is
    // parallel sampling, not a consolation prize.
    let attempt = attempt_over(
        vec![
            ("prover".to_owned(), vec![answers("one way")]),
            ("refuter".to_owned(), vec![answers("another way")]),
        ],
        &["prover", "refuter"],
    );
    let mut state = planned("goal", &[("one", "first"), ("two", "second")]);
    state.unproductive = Thresholds::default().stuck;

    let briefs = attempt.briefs(&state, Route::Diversify, &[]);

    assert_eq!(briefs.len(), 2);
    assert_eq!(briefs[0].0, "prover");
    assert_eq!(briefs[1].0, "refuter");
    assert_eq!(briefs[0].1.task, briefs[1].1.task);
}

#[test]
fn a_retry_route_gives_each_outstanding_task_a_specialist() {
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("one way")])],
        &["prover"],
    );
    let state = planned("goal", &[("one", "first"), ("two", "second")]);

    let briefs = attempt.briefs(&state, Route::Retry, &[]);

    assert_eq!(briefs.len(), 2);
    assert_eq!(briefs[0].1.task, "first");
    assert_eq!(briefs[1].1.task, "second");
}

#[test]
fn an_empty_board_briefs_the_goal_itself() {
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("something")])],
        &["prover"],
    );
    let state = LoopState::new("the goal as given");

    let retried = attempt.briefs(&state, Route::Retry, &[]);
    let diversified = attempt.briefs(&state, Route::Diversify, &[]);

    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].1.task, "the goal as given");
    assert_eq!(diversified[0].1.task, "the goal as given");
}

#[test]
fn every_brief_carries_the_steer_the_last_lesson_and_the_directives() {
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("ok")])],
        &["prover"],
    );
    let mut state = planned("goal", &[("one", "first")]);
    state.steer = "narrow the claim".to_owned();
    state.lessons = vec!["an old lesson".to_owned(), "read the error first".to_owned()];

    let briefs = attempt.briefs(
        &state,
        Route::Retry,
        &[Note::new("operator", "stop guessing")],
    );

    let context = &briefs[0].1.context;
    assert!(context.contains("steer: narrow the claim"));
    assert!(context.contains("lesson: read the error first"));
    assert!(context.contains("directive from operator: stop guessing"));
    // Only the last lesson, so the brief does not grow without bound.
    assert!(!context.contains("an old lesson"));
}

#[test]
fn a_directive_reaches_the_next_attempt_and_the_report() {
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("ok")])],
        &["prover"],
    );
    let thresholds = Thresholds::default();
    assert!(
        attempt
            .mailbox()
            .post(Note::new("operator", "try the other basis"))
            .is_accepted()
    );

    let state = attempt
        .run(planned("goal", &[("one", "first")]), context(0, &thresholds))
        .expect("the attempt runs")
        .into_state();

    let report: AttemptReport = serde_json::from_str(&state.last_attempt).expect("a report");
    assert_eq!(report.directives, vec!["try the other basis".to_owned()]);
    // Drained, so the next pass does not act on it twice.
    assert!(attempt.mailbox().is_empty());
}

#[test]
fn a_directive_posted_to_a_full_mailbox_is_dropped_and_recorded() {
    // Dropping is the only one of the three failure modes that leaves the loop
    // running: an unbounded queue turns a slow consumer into unbounded memory,
    // and a blocking send turns one into a stalled loop.
    #[derive(Debug, Default)]
    struct Collected(std::sync::Mutex<Vec<String>>);

    impl Sink for Collected {
        fn emit(&self, event: &Event) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(event.kind().to_owned());
            }
        }
    }

    let sink = Arc::new(Collected::default());
    let mailbox = Mailbox::observed(1, Arc::new(SinkDrops::new(sink.clone())));

    assert!(mailbox.post(Note::new("operator", "first")).is_accepted());
    let refused = mailbox.post(Note::new("operator", "second"));

    assert!(!refused.is_accepted());
    assert_eq!(mailbox.drops(), 1);
    assert_eq!(mailbox.len(), 1);
    assert_eq!(mailbox.collect(), vec![Note::new("operator", "first")]);
    assert!(
        sink.0
            .lock()
            .expect("the sink is never poisoned")
            .iter()
            .any(|kind| kind == "note_dropped")
    );
}

#[test]
fn the_inline_dispatcher_serves_each_role_its_own_queue_in_order() {
    let set = DelegateSet::of(["prover"]);
    let dispatcher = Inline::of(
        set,
        [(
            "prover".to_owned(),
            vec![answers("first"), answers("second")],
        )],
    );

    let one = dispatcher
        .dispatch(vec![("prover".to_owned(), Brief::new("a"))])
        .expect("dispatched");
    let two = dispatcher
        .dispatch(vec![("prover".to_owned(), Brief::new("b"))])
        .expect("dispatched");
    let three = dispatcher
        .dispatch(vec![("prover".to_owned(), Brief::new("c"))])
        .expect("dispatched");

    assert_eq!(one[0].reply.as_deref(), Some("first"));
    assert_eq!(two[0].reply.as_deref(), Some("second"));
    // The last entry repeats rather than running out: a loop runs an unknown
    // number of passes, and an exhausted script would turn a routing question
    // into a fixture-length question.
    assert_eq!(three[0].reply.as_deref(), Some("second"));
    assert_eq!(dispatcher.served("prover"), 3);
    assert_eq!(dispatcher.served("refuter"), 0);
}

#[test]
fn a_declared_role_with_no_script_is_refused_rather_than_silently_empty() {
    let set = DelegateSet::of(["prover"]);
    let dispatcher = Inline::of(set, [("prover".to_owned(), Vec::new())]);

    let refused = dispatcher.dispatch(vec![("prover".to_owned(), Brief::new("a"))]);

    assert!(matches!(refused, Err(Error::SpawnRefused { ref role, .. }) if role == "prover"));
}

#[test]
fn a_declared_role_absent_from_the_script_names_itself() {
    let set = DelegateSet::of(["prover", "refuter"]);
    let dispatcher = Inline::of(set, [("prover".to_owned(), vec![answers("ok")])]);

    let refused = dispatcher.dispatch(vec![("refuter".to_owned(), Brief::new("a"))]);

    assert!(matches!(refused, Err(Error::UndeclaredDelegate { ref name }) if name == "refuter"));
}

#[test]
fn an_inline_outcome_matches_the_asynchronous_harnesss_outcome() {
    // One mapping, one spelling. Two would be how a test starts agreeing with
    // a bug.
    let scripted = Scripted::Capped {
        artifacts: vec![Artifact::new("notes.md", "partial")],
    };
    let brief = Brief::new("survey it");

    let direct = scripted.outcome(brief.clone());
    let dispatched = Inline::of(
        DelegateSet::of(["prover"]),
        [("prover".to_owned(), vec![scripted])],
    )
    .dispatch(vec![("prover".to_owned(), brief)])
    .expect("dispatched");

    assert_eq!(dispatched[0], direct);
}

// -------------------------------------------------------------- the report

#[test]
fn report_is_the_sole_author_of_the_final_answer() {
    let report = Report::new(Arc::new(Summarize));
    let thresholds = Thresholds::default();
    let mut state = planned("prove the bound", &[("one", "first"), ("two", "second")]);
    state
        .board
        .settle(&id("one"), TaskStatus::Discharged, 1)
        .expect("exists");
    state.attempts = 4;
    state.passes = 3;
    state.lessons = vec!["read the error first".to_owned()];

    let after = report
        .run(state, context(3, &thresholds))
        .expect("the report step runs")
        .into_state();

    assert!(after.answer.contains("prove the bound"));
    assert!(after.answer.contains("1 of 2 tasks discharged"));
    assert!(after.answer.contains("[discharged] first"));
    assert!(after.answer.contains("[open] second"));
    assert!(after.answer.contains("read the error first"));
}

#[test]
fn a_run_with_nothing_learned_reports_no_lessons_section() {
    let composed = Summarize
        .compose(&LoopState::new("a goal"))
        .expect("the reference composer never fails");

    assert!(composed.contains("0 of 0 tasks discharged"));
    assert!(!composed.contains("Learned:"));
}

#[test]
fn the_three_steps_answer_to_the_names_the_graph_addresses() {
    let plan = Plan::new(Arc::new(FixedPlan::default()));
    let report = Report::new(Arc::new(Summarize));
    let attempt = attempt_over(
        vec![("prover".to_owned(), vec![answers("ok")])],
        &["prover"],
    );

    assert_eq!(plan.name(), crate::step::STEP_PLAN);
    assert_eq!(attempt.name(), crate::step::STEP_ATTEMPT);
    assert_eq!(report.name(), crate::step::STEP_REPORT);
}
