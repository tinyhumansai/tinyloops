//! Unit tests for the loop's accumulator and its fold.
//!
//! Three things are pinned here, and each one is pinned because its failure is
//! silent:
//!
//! - the **wire form**, because the field names are what the graph's jq
//!   addresses, so a rename is a decode error at runtime rather than a compile
//!   error here;
//! - **commutativity**, because update ordering is arbitrary and an
//!   order-dependent fold produces a different run on a different day with
//!   nothing to show for it;
//! - **idempotence**, because the engine's fold is at-least-once and a replayed
//!   activation would otherwise double-count.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::*;
use crate::Error;
use crate::policy::Judgement;

/// A fully populated accumulator, so the wire form pins every field rather than
/// only the ones that differ from their defaults.
fn populated() -> LoopState {
    LoopState {
        goal: "land the change".to_string(),
        passes: 3,
        attempts: 4,
        unproductive: 1,
        blocked: 2,
        computational: 3,
        unverified: 4,
        restarts: 1,
        established: 5,
        banked: 6,
        solved: true,
        expired: false,
        last_attempt: "ran the suite".to_string(),
        lessons: vec!["read the error first".to_string()],
        steer: "narrow the change".to_string(),
        scores: vec![7, 9],
        judged: Judgement::Steer,
        board: board(),
        profile: crate::policy::LoopProfile::of(crate::presets::Preset::Cautious),
        answer: "the bound holds".to_string(),
    }
}

/// A one-task board, so the pinned wire form covers the nested field rather
/// than only its empty default.
fn board() -> crate::orchestrate::TaskBoard {
    let mut board = crate::orchestrate::TaskBoard::new();
    board
        .add(crate::orchestrate::Task::new(
            crate::orchestrate::TaskId::new("bound-the-error").unwrap(),
            "bound the error term",
            "a proved bound",
            2,
        ))
        .unwrap();
    board.planned(2);
    board
}

#[test]
fn the_wire_form_is_pinned() {
    assert_eq!(
        serde_json::to_value(populated()).unwrap(),
        json!({
            "goal": "land the change",
            "passes": 3,
            "attempts": 4,
            "unproductive": 1,
            "blocked": 2,
            "computational": 3,
            "unverified": 4,
            "restarts": 1,
            "established": 5,
            "banked": 6,
            "solved": true,
            "expired": false,
            "last_attempt": "ran the suite",
            "lessons": ["read the error first"],
            "steer": "narrow the change",
            "scores": [7, 9],
            "judged": "steer",
            "board": {
                "tasks": [{
                    "id": "bound-the-error",
                    "statement": "bound the error term",
                    "criterion": "a proved bound",
                    "status": "open",
                    "touched": 2,
                }],
                "planned_at": 2,
            },
            "profile": {
                "revision": 0,
                "thresholds": {
                    "max_attempts": 8,
                    "stuck": 2,
                    "blocked": 2,
                    "computational": 2,
                    "unverified": 1,
                    "max_restarts": 2,
                    "plan_interval": 3,
                },
                "origin": "cautious",
            },
            "answer": "the bound holds",
        })
    );
}

#[test]
fn the_wire_form_round_trips() {
    let encoded = serde_json::to_value(populated()).unwrap();
    assert_eq!(
        serde_json::from_value::<LoopState>(encoded).unwrap(),
        populated()
    );
}

#[test]
fn an_older_accumulator_still_deserializes() {
    // Every field defaults, so an accumulator written before a field existed
    // reads back rather than failing the run that resumed it.
    let older: LoopState = serde_json::from_value(json!({
        "goal": "land the change",
        "passes": 2,
    }))
    .unwrap();

    assert_eq!(older.goal, "land the change");
    assert_eq!(older.passes, 2);
    assert_eq!(older.established, 0);
    assert_eq!(older.judged, Judgement::Proceed);
}

#[test]
fn an_empty_accumulator_deserializes_to_the_default() {
    assert_eq!(
        serde_json::from_value::<LoopState>(json!({})).unwrap(),
        LoopState::default()
    );
}

#[test]
fn an_unknown_field_is_ignored() {
    let state: LoopState = serde_json::from_value(json!({ "invented_later": 3 })).unwrap();
    assert_eq!(state, LoopState::default());
}

#[test]
fn a_new_run_starts_on_its_goal_with_nothing_counted() {
    let state = LoopState::new("land the change");
    assert_eq!(state.goal, "land the change");
    assert_eq!(
        state,
        LoopState {
            goal: "land the change".to_string(),
            ..LoopState::default()
        }
    );
}

#[test]
fn applying_a_passs_own_delta_reproduces_that_pass() {
    let base = populated();
    let mut next = base.clone();
    next.passes = 4;
    next.unproductive = 0;
    next.established = 7;
    next.expired = true;

    assert_eq!(base.apply(&[next.delta_from(&base)]), next);
}

#[test]
fn a_reset_and_an_increment_compose_in_one_superstep() {
    let mut base = LoopState::new("goal");
    base.unproductive = 1;

    // One arm found the pass productive and zeroed the counter.
    let mut productive = base.clone();
    productive.unproductive = 0;
    productive.established = 1;

    // Another arm, from the same base, recorded a restart and counted the pass
    // against the run.
    let mut restarted = base.clone();
    restarted.unproductive = 2;
    restarted.restarts = 1;

    let merged = base.apply(&[productive.delta_from(&base), restarted.delta_from(&base)]);

    // Neither arm was lost: -1 and +1 both landed.
    assert_eq!(merged.unproductive, 1);
    assert_eq!(merged.established, 1);
    assert_eq!(merged.restarts, 1);
}

/// Every permutation of `values`, by repeated rotation of each prefix.
fn permutations(values: &[Delta]) -> Vec<Vec<Delta>> {
    if values.len() <= 1 {
        return vec![values.to_vec()];
    }
    let mut out = Vec::new();
    for index in 0..values.len() {
        let mut rest = values.to_vec();
        let head = rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, head);
            out.push(tail);
        }
    }
    out
}

#[test]
fn the_fold_is_commutative() {
    let base = populated();

    let deltas: Vec<Delta> = [
        LoopState {
            unproductive: 0,
            established: 9,
            ..base.clone()
        },
        LoopState {
            unproductive: 5,
            restarts: 4,
            ..base.clone()
        },
        LoopState {
            banked: 0,
            solved: false,
            ..base.clone()
        },
        LoopState {
            attempts: 9,
            expired: true,
            ..base.clone()
        },
    ]
    .iter()
    .map(|arm| arm.delta_from(&base))
    .collect();

    let orderings = permutations(&deltas);
    assert_eq!(orderings.len(), 24);

    let expected = base.apply(&deltas);
    for ordering in orderings {
        assert_eq!(base.apply(&ordering), expected);
    }
}

#[test]
fn a_replayed_pass_changes_nothing() {
    // The engine's fold is at-least-once, so the same activation can apply
    // twice. It survives that because a pass's update is an assignment: once
    // the base already holds the pass's result, the movement to it is zero.
    let base = populated();
    let mut next = base.clone();
    next.passes = 4;
    next.attempts = 5;
    next.unproductive = 0;

    let once = base.apply(&[next.delta_from(&base)]);
    let replayed = once.apply(&[next.delta_from(&once)]);

    assert_eq!(replayed, once);
    assert_eq!(replayed.passes, 4);
}

#[test]
fn a_set_flag_wins_over_a_concurrent_unset_one() {
    let base = LoopState::new("goal");

    let mut solved = base.clone();
    solved.solved = true;
    let mut unsolved = base.clone();
    unsolved.solved = false;

    let votes = [solved.delta_from(&base), unsolved.delta_from(&base)];
    assert!(base.apply(&votes).solved);
    assert!(base.apply(&[votes[1], votes[0]]).solved);
}

#[test]
fn a_flag_nobody_voted_on_keeps_its_value() {
    let mut base = LoopState::new("goal");
    base.expired = true;

    let mut arm = base.clone();
    arm.passes = 1;

    assert!(base.apply(&[arm.delta_from(&base)]).expired);
}

#[test]
fn a_flag_can_be_cleared_when_nothing_sets_it() {
    let mut base = LoopState::new("goal");
    base.solved = true;

    let mut arm = base.clone();
    arm.solved = false;

    assert!(!base.apply(&[arm.delta_from(&base)]).solved);
}

#[test]
fn counters_saturate_rather_than_wrapping() {
    let base = LoopState {
        passes: u32::MAX,
        attempts: u32::MAX,
        ..LoopState::new("goal")
    };
    let step = Delta {
        passes: 1,
        attempts: i64::MAX,
        ..Delta::default()
    };

    let saturated = base.apply(&[step, step, step]);
    assert_eq!(saturated.passes, u32::MAX);
    assert_eq!(saturated.attempts, u32::MAX);
}

#[test]
fn counters_stop_at_zero_rather_than_going_negative() {
    let base = LoopState::new("goal");
    let step = Delta {
        unproductive: i64::MIN,
        blocked: -5,
        ..Delta::default()
    };

    let floored = base.apply(&[step, step]);
    assert_eq!(floored.unproductive, 0);
    assert_eq!(floored.blocked, 0);
}

#[test]
fn a_delta_of_no_change_is_the_default() {
    let base = populated();
    assert_eq!(base.delta_from(&base), Delta::default());
    assert_eq!(base.apply(&[]), base);
}

#[test]
fn the_narrative_fields_are_carried_from_the_base() {
    let base = populated();
    let mut arm = base.clone();
    arm.goal = "something else".to_string();
    arm.last_attempt = "different".to_string();
    arm.lessons = vec!["a lesson".to_string()];
    arm.steer = "elsewhere".to_string();
    arm.scores = vec![1];
    arm.judged = Judgement::Restart;
    arm.passes = base.passes + 1;

    let merged = base.apply(&[arm.delta_from(&base)]);

    // The counter moved; the text did not. `apply` folds counters only —
    // narrative travels as a `Contribution`, arbitrated by `merge`.
    assert_eq!(merged.passes, base.passes + 1);
    assert_eq!(merged.goal, base.goal);
    assert_eq!(merged.last_attempt, base.last_attempt);
    assert_eq!(merged.lessons, base.lessons);
    assert_eq!(merged.steer, base.steer);
    assert_eq!(merged.scores, base.scores);
    assert_eq!(merged.judged, base.judged);
}

#[test]
fn merge_carries_each_arms_narrative_into_the_next_pass() {
    let base = LoopState::new("goal");

    let mut reflection = Contribution::new("reflect");
    reflection.lesson = Some("the oracle disagreed".to_owned());
    let mut judge = Contribution::new("judge");
    judge.score = Some(4);
    judge.steer = Some("check the bound first".to_owned());
    judge.judged = Some(Judgement::Steer);

    let merged = base
        .merge(&[], &[reflection, judge])
        .expect("distinct fields do not contest");

    assert_eq!(merged.lessons, ["the oracle disagreed"]);
    assert_eq!(merged.scores, [4]);
    assert_eq!(merged.steer, "check the bound first");
    assert_eq!(merged.judged, Judgement::Steer);
}

#[test]
fn merge_is_order_independent_over_contributions() {
    let base = LoopState::new("goal");

    let mut reflection = Contribution::new("reflect");
    reflection.lesson = Some("a lesson".to_owned());
    let mut judge = Contribution::new("judge");
    judge.score = Some(3);

    let forwards = base
        .merge(&[], &[reflection.clone(), judge.clone()])
        .expect("distinct fields do not contest");
    let backwards = base
        .merge(&[], &[judge, reflection])
        .expect("distinct fields do not contest");

    assert_eq!(forwards, backwards);
}

#[test]
fn merge_refuses_two_arms_writing_the_same_field() {
    let base = LoopState::new("goal");

    let mut first = Contribution::new("reflect");
    first.steer = Some("one way".to_owned());
    let mut second = Contribution::new("judge");
    second.steer = Some("the other way".to_owned());

    // Picking a winner here would be arrival-order dependence wearing a
    // merge's clothes, so the wiring mistake is reported instead.
    let error = base
        .merge(&[], &[first, second])
        .expect_err("two arms wrote steer");

    assert_eq!(
        error,
        Error::ContestedField {
            field: "steer",
            held_by: "reflect",
            also: "judge",
        }
    );
}

#[test]
fn merge_folds_counters_and_narrative_in_one_pass() {
    let mut base = LoopState::new("goal");
    base.unproductive = 1;

    let mut productive = base.clone();
    productive.unproductive = 0;

    let mut reflection = Contribution::new("reflect");
    reflection.lesson = Some("it worked".to_owned());

    let merged = base
        .merge(&[productive.delta_from(&base)], &[reflection])
        .expect("one writer per field");

    assert_eq!(merged.unproductive, 0);
    assert_eq!(merged.lessons, ["it worked"]);
}

#[test]
fn an_arm_that_contributes_nothing_is_ordinary() {
    let quiet = Contribution::new("patterns");
    assert!(quiet.is_empty());

    let base = LoopState::new("goal");
    let merged = base.merge(&[], &[quiet]).expect("silence contests nothing");
    assert_eq!(merged, base);
}
