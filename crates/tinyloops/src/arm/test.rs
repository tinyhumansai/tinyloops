//! Unit tests for the arm declaration, its two derived edge sets, and the fold.
//!
//! What is pinned here, and why each is worth a test rather than a comment:
//!
//! - **one list, both edge sets** — the fan-out and the fold are asserted to
//!   name the *same* arms, derived from one declaration. As two facts they
//!   drift silently: an arm in the fan-out but not the fold runs, costs its
//!   budget, and changes nothing.
//! - **the fold composes and commutes** — a reset and an increment from the
//!   same base both land, in any order. A last-writer-wins merge passes every
//!   other test in this file and fails that one.
//! - **exactly one arm may conclude** — two means the run's outcome depends on
//!   which finished first.
//!
//! Invariant 3 — an arm reads the previous step, never the accumulator — is
//! only partly testable here: [`upstream_address`] is asserted to build the
//! upstream node's address, but whether an arm was *wired* to it is a property
//! of the emitted graph. That test needs an attempt mock whose answer varies
//! per call, because a constant mock reports the same value one pass behind as
//! it does current, and passes either way.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::policy::{Judgement, Thresholds};
use crate::state::Contribution;
use crate::step::{NoWrite, StepContext};

/// An arm that moves one counter and files one narrative field.
///
/// Everything an arm does that the merge cares about, parameterized so a test
/// can build several that differ only where it means them to.
struct Fake {
    name: &'static str,
    concludes: bool,
    unproductive: Option<u32>,
    lesson: Option<&'static str>,
}

impl Fake {
    fn plain(name: &'static str) -> Arc<dyn Arm> {
        Arc::new(Self {
            name,
            concludes: false,
            unproductive: None,
            lesson: None,
        })
    }

    fn concluding(name: &'static str) -> Arc<dyn Arm> {
        Arc::new(Self {
            name,
            concludes: true,
            unproductive: None,
            lesson: None,
        })
    }
}

impl Arm for Fake {
    fn name(&self) -> &'static str {
        self.name
    }

    fn may_conclude(&self) -> bool {
        self.concludes
    }

    fn evaluate(
        &self,
        base: &LoopState,
        report: &Value,
        ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome> {
        let mut outcome = ArmOutcome::unchanged(self.name, base);
        if let Some(unproductive) = self.unproductive {
            outcome.state.unproductive = unproductive;
        }
        outcome.contribution.lesson = self.lesson.map(str::to_string);
        // The report is what an arm reads, and the pass number comes from the
        // context: neither is the loop head's accumulator.
        outcome.contribution.last_attempt = report
            .get("report")
            .and_then(Value::as_str)
            .map(|text| format!("{text} on pass {}", ctx.pass()));
        Ok(outcome)
    }
}

fn set() -> ArmSet {
    ArmSet::new(vec![Fake::concluding("reflect"), Fake::plain("judge")]).unwrap()
}

fn base() -> LoopState {
    let mut base = LoopState::new("goal");
    base.unproductive = 3;
    base.passes = 2;
    base
}

/// An outcome from `arm` that sets `unproductive` to `value`.
fn moves(arm: &'static str, base: &LoopState, value: u32) -> ArmOutcome {
    let mut outcome = ArmOutcome::unchanged(arm, base);
    outcome.state.unproductive = value;
    outcome
}

#[test]
fn declares_its_arms_in_order() {
    let set = set();

    assert_eq!(set.names(), ["reflect", "judge"]);
    assert_eq!(set.len(), 2);
    assert!(!set.is_empty());
    assert_eq!(set.arms().len(), 2);
    assert_eq!(set.concluding(), Some("reflect"));
}

#[test]
fn an_empty_arm_set_is_a_construction_error() {
    assert_eq!(ArmSet::new(vec![]).unwrap_err(), Error::EmptyArmSet);
}

#[test]
fn duplicate_arm_names_are_a_construction_error() {
    assert_eq!(
        ArmSet::new(vec![Fake::plain("judge"), Fake::plain("judge")]).unwrap_err(),
        Error::DuplicateArm {
            name: "judge".to_string()
        },
    );
}

#[test]
fn two_concluding_arms_are_a_construction_error() {
    // Two arms able to end a run means the outcome depends on which of them
    // finished first, which is the one thing every other rule here removes.
    assert_eq!(
        ArmSet::new(vec![
            Fake::concluding("reflect"),
            Fake::concluding("oracle")
        ])
        .unwrap_err(),
        Error::AmbiguousConclusion {
            first: "reflect",
            second: "oracle",
        },
    );
}

#[test]
fn a_set_with_no_concluding_arm_is_allowed() {
    let set = ArmSet::new(vec![Fake::plain("judge")]).unwrap();

    assert_eq!(set.concluding(), None);
}

#[test]
fn the_fan_out_and_the_fold_name_the_same_arms() {
    let set = set();

    let fanned: Vec<_> = set.fan_out("attempt").into_iter().map(|e| e.to).collect();
    // Compared through `upstream_address` rather than by stripping a literal
    // prefix and suffix: a test that re-spells the address format is a second
    // copy of it, and the two drift the moment the format changes.
    let folded: Vec<_> = set.fold_inputs();
    let expected_fold: Vec<_> = fanned.iter().map(|arm| upstream_address(arm)).collect();
    let converged: Vec<_> = set.converge("merge").into_iter().map(|e| e.from).collect();

    assert_eq!(folded, expected_fold);

    // One list, three views. Asserted equal because "every arm converges" and
    // "every arm is folded" must be one fact; as two they drift, and the drift
    // costs an arm's budget and changes nothing.
    assert_eq!(fanned, converged);
}

#[test]
fn removing_an_arm_removes_it_from_both_edge_sets_and_the_fold() {
    let three = ArmSet::new(vec![
        Fake::plain("reflect"),
        Fake::plain("judge"),
        Fake::plain("oracle"),
    ])
    .unwrap();
    let two = ArmSet::new(vec![Fake::plain("reflect"), Fake::plain("judge")]).unwrap();

    assert!(three.fan_out("attempt").len() == 3 && three.converge("merge").len() == 3);
    assert!(
        !two.fan_out("attempt")
            .contains(&Edge::new("attempt", "oracle"))
    );
    assert!(
        !two.converge("merge")
            .contains(&Edge::new("oracle", "merge"))
    );
    assert!(!two.fold_inputs().contains(&upstream_address("oracle")));
}

#[test]
fn an_arms_input_is_the_upstream_nodes_payload() {
    // Invariant 3's helper. `=.nodes.<loop>.state` is the address it exists to
    // keep out of an arm's input: the head folds at the top of a pass, so
    // mid-body that slot is one pass behind.
    assert_eq!(upstream_address("attempt"), "=nodes.attempt.item.json");
    assert!(!upstream_address("attempt").contains(".state"));
}

#[test]
fn an_arms_input_resolves_against_a_completed_node() {
    // The assertion above only pins the *string*. This one runs it through the
    // engine, because the failure being guarded is silent: the `nodes` scope
    // holds `item` and `items` and no `output`, and an address naming a key the
    // scope lacks evaluates to `null` rather than erroring. An arm wired to one
    // reads nothing while the run reports success.
    let scope = serde_json::json!({
        "nodes": {
            "attempt": {
                "item": { "json": { "report": "two routes agreed" } },
                "items": [{ "json": { "report": "two routes agreed" } }],
            }
        }
    });

    let resolved = tinyflows::expr::evaluate(
        &serde_json::Value::String(upstream_address("attempt")),
        &scope,
    );

    assert_eq!(
        resolved,
        serde_json::json!({ "report": "two routes agreed" })
    );
    assert!(!resolved.is_null(), "a null here is the silent failure");
}

#[test]
fn a_hyphenated_node_id_still_addresses_its_payload() {
    // The dotted-path form is resolved by a segment walk rather than by jq, so
    // the hyphen is a literal key character. Under jq it would be subtraction,
    // and the address would resolve to null.
    let scope = serde_json::json!({
        "nodes": { "eval-judge": { "item": { "json": { "score": 4 } }, "items": [] } }
    });

    let resolved = tinyflows::expr::evaluate(
        &serde_json::Value::String(upstream_address("eval-judge")),
        &scope,
    );

    assert_eq!(resolved, serde_json::json!({ "score": 4 }));
}

#[test]
fn an_arm_evaluates_from_the_report_it_was_handed() {
    let thresholds = Thresholds::default();
    let arm = Fake::plain("judge");
    let outcome = arm
        .evaluate(
            &base(),
            &json!({ "report": "tried the fast path" }),
            StepContext::observing(2, &thresholds),
        )
        .unwrap();

    assert_eq!(outcome.arm(), "judge");
    assert_eq!(
        outcome.contribution.last_attempt.as_deref(),
        Some("tried the fast path on pass 2"),
    );
}

#[test]
fn a_reset_and_an_increment_compose_from_the_same_base() {
    let set = set();
    let base = base();

    // One arm zeroes the counter (−3), another increments it (+1). A
    // last-writer-wins merge yields 0 or 4 depending on arrival order; the
    // delta fold yields 1 either way.
    let merged = set
        .merge(
            &base,
            vec![moves("reflect", &base, 0), moves("judge", &base, 4)],
        )
        .unwrap();

    assert_eq!(merged.unproductive, 1);
}

#[test]
fn the_fold_is_commutative_over_arm_order() {
    let set = set();
    let base = base();

    let forward = set
        .merge(
            &base,
            vec![moves("reflect", &base, 0), moves("judge", &base, 4)],
        )
        .unwrap();
    let backward = set
        .merge(
            &base,
            vec![moves("judge", &base, 4), moves("reflect", &base, 0)],
        )
        .unwrap();

    assert_eq!(forward, backward);
}

#[test]
fn the_fold_is_commutative_over_contributions_too() {
    let set = set();
    let base = base();

    let lesson = |arm: &'static str| {
        let mut outcome = ArmOutcome::unchanged(arm, &base);
        outcome.contribution.lesson = Some(format!("{arm} learned something"));
        outcome
    };
    let scored = |arm: &'static str| {
        let mut outcome = ArmOutcome::unchanged(arm, &base);
        outcome.contribution.score = Some(4);
        outcome.contribution.judged = Some(Judgement::Steer);
        outcome
    };

    let forward = set
        .merge(&base, vec![lesson("reflect"), scored("judge")])
        .unwrap();
    let backward = set
        .merge(&base, vec![scored("judge"), lesson("reflect")])
        .unwrap();

    assert_eq!(forward, backward);
    assert_eq!(forward.lessons, ["reflect learned something"]);
    assert_eq!(forward.scores, [4]);
    assert_eq!(forward.judged, Judgement::Steer);
}

#[test]
fn every_permutation_of_four_arms_folds_to_one_answer() {
    let arms = ["a", "b", "c", "d"];
    let set = ArmSet::new(arms.iter().copied().map(Fake::plain).collect()).unwrap();
    let base = base();

    let outcome = |index: usize| {
        let mut outcome = moves(arms[index], &base, u32::try_from(index).unwrap());
        outcome.state.banked = u32::try_from(index).unwrap();
        outcome
    };

    // Exhaustive over the 24 permutations of four values rather than
    // generative: it covers the property with no new dependency.
    let mut answers = Vec::new();
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    if [a, b, c, d]
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != 4
                    {
                        continue;
                    }
                    answers.push(
                        set.merge(&base, vec![outcome(a), outcome(b), outcome(c), outcome(d)])
                            .unwrap(),
                    );
                }
            }
        }
    }

    assert_eq!(answers.len(), 24);
    assert!(answers.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn distinct_narrative_fields_from_different_arms_all_land() {
    let set = set();
    let base = base();

    let mut reflect = ArmOutcome::unchanged("reflect", &base);
    reflect.contribution.lesson = Some("the fast path is a dead end".to_string());
    let mut judge = ArmOutcome::unchanged("judge", &base);
    judge.contribution.steer = Some("try the slow path".to_string());
    judge.contribution.score = Some(2);

    let merged = set.merge(&base, vec![reflect, judge]).unwrap();

    assert_eq!(merged.lessons, ["the fast path is a dead end"]);
    assert_eq!(merged.steer, "try the slow path");
    assert_eq!(merged.scores, [2]);
}

#[test]
fn two_arms_writing_one_narrative_field_is_refused() {
    let set = set();
    let base = base();

    let mut reflect = ArmOutcome::unchanged("reflect", &base);
    reflect.contribution.steer = Some("left".to_string());
    let mut judge = ArmOutcome::unchanged("judge", &base);
    judge.contribution.steer = Some("right".to_string());

    // Sorted by arm name before folding, so the report names the same two arms
    // whichever order they arrived in.
    assert_eq!(
        set.merge(&base, vec![reflect.clone(), judge.clone()])
            .unwrap_err(),
        Error::ContestedField {
            field: "steer",
            held_by: "judge",
            also: "reflect",
        },
    );
    assert_eq!(
        set.merge(&base, vec![judge, reflect]).unwrap_err(),
        Error::ContestedField {
            field: "steer",
            held_by: "judge",
            also: "reflect",
        },
    );
}

#[test]
fn folding_an_undeclared_arm_is_refused() {
    let set = set();
    let base = base();

    assert_eq!(
        set.merge(
            &base,
            vec![
                ArmOutcome::unchanged("reflect", &base),
                ArmOutcome::unchanged("judge", &base),
                ArmOutcome::unchanged("oracle", &base),
            ],
        )
        .unwrap_err(),
        Error::UnknownArm { name: "oracle" },
    );
}

#[test]
fn a_declared_arm_missing_from_the_fold_is_refused() {
    let set = set();
    let base = base();

    // Invariant 6 at the barrier: an arm that ran and was not folded costs its
    // budget and changes nothing, which is exactly the silent drift one list
    // exists to prevent.
    assert_eq!(
        set.merge(&base, vec![ArmOutcome::unchanged("reflect", &base)])
            .unwrap_err(),
        Error::ArmNotFolded { name: "judge" },
    );
}

#[test]
fn an_unchanged_outcome_moves_nothing() {
    let base = base();
    let outcome = ArmOutcome::unchanged("judge", &base);

    assert_eq!(outcome.state, base);
    assert!(outcome.contribution.is_empty());
    assert_eq!(outcome.contribution, Contribution::new("judge"));
}

#[test]
fn debug_rendering_names_the_declared_arms() {
    let rendered = format!("{:?}", set());

    assert!(rendered.contains("reflect"));
    assert!(rendered.contains("judge"));
    assert!(format!("{:?}", Edge::new("attempt", "judge")).contains("attempt"));
}
