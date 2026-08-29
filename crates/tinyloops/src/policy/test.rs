//! Unit tests for the policy module, and the parity sweep between the Rust
//! router and the jq it generates.
//!
//! # What parity proves
//!
//! [`route`] and [`ladder`] are two implementations of one decision, written in
//! two languages because the branch is taken inside a workflow graph. The
//! sweeps below assert that for every combination of the counters either side
//! reads, both produce the same answer. That is a proof about the
//! *translation*: it catches an arm typed in the wrong order in the jq, a `>`
//! where the Rust has `>=`, a route name spelled differently on the two sides,
//! and a program that stopped compiling.
//!
//! # What it does not prove
//!
//! It says nothing about whether the shared answer is *right*. Both sides read
//! the same [`Thresholds`], so a badly chosen threshold is badly chosen in both
//! and agrees with itself perfectly. Parity is a consistency check, not a
//! validation of the policy; the bets themselves are argued in the doc comments
//! on [`Thresholds`] and are revised there, not here.
//!
//! # Why the sweep is exhaustive rather than sampled
//!
//! There are only six counters on each side and each one matters over a range
//! of a few values, so the whole input space is small enough to enumerate. The
//! ranges are derived from the [`Thresholds`] under test — `0` through one past
//! each threshold — because a fixed range that stopped short of a raised
//! threshold would leave the interesting room untested and report the silence
//! as agreement.
//!
//! # Why every result is checked against null
//!
//! Under this engine a compile error, a run error, non-JSON output, and empty
//! output all yield `Value::Null`, and `null` is falsey. A broken program is
//! therefore indistinguishable from a false condition unless something asserts
//! the result has the shape it should — so every evaluation here does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use tinyflows::expr;

use super::{
    Amendment, Autonomy, Bounds, CapField, Change, Judgement, LoopProfile, Outcome, Range, Route,
    ThresholdField, Thresholds, expr_scope, is_terminal, ladder, route, terminal_condition,
};
use crate::Error;
use crate::state::LoopState;

/// A hyphenated id, because a node id is a literal key in an expression rather
/// than a jq subtraction and the scope has to survive one.
const LOOP_ID: &str = "goal-loop";

/// The threshold sets every sweep runs under: **every shipped preset**, plus a
/// set deliberately different in every field so a sweep cannot pass by
/// accidentally agreeing on the numbers it was written against.
///
/// Reading the presets from [`Preset::ALL`] rather than listing them is the
/// point. A preset is a generated ladder, and a generated ladder nobody proved
/// against [`route`] is a routing decision nobody checked. Deriving the sweep
/// from the same constant the presets are published from means a preset cannot
/// be added without being swept.
fn threshold_sets() -> Vec<Thresholds> {
    crate::presets::Preset::ALL
        .into_iter()
        .map(crate::presets::Preset::thresholds)
        .chain(std::iter::once(Thresholds {
            max_attempts: 3,
            stuck: 1,
            blocked: 3,
            computational: 4,
            unverified: 1,
            max_restarts: 1,
            plan_interval: 2,
        }))
        .collect()
}

#[test]
fn the_sweep_covers_every_shipped_preset() {
    // Asserted rather than assumed: the sweeps below iterate `threshold_sets`,
    // and this is what makes "every preset is swept" a checked fact rather than
    // a property of how that function happens to be written today.
    let swept = threshold_sets();
    for preset in crate::presets::Preset::ALL {
        assert!(
            swept.contains(&preset.thresholds()),
            "{preset} is not in the parity sweep"
        );
    }
}

/// `0` through one past `threshold`, so every sweep reaches past the point the
/// threshold fires.
fn upto(threshold: u32) -> std::ops::RangeInclusive<u32> {
    0..=threshold.saturating_add(1)
}

/// How many values [`upto`] yields, for computing a sweep's expected case count.
fn span(threshold: u32) -> usize {
    upto(threshold).count()
}

/// A `state` routing under `thresholds`.
///
/// Both sides read the thresholds out of the accumulator now, so a sweep sets
/// them there rather than passing them alongside. That is the property under
/// test as much as a convenience: a test that could hand the router one set and
/// the ladder another would be testing a configuration no run can reach.
fn under(state: LoopState, thresholds: Thresholds) -> LoopState {
    LoopState {
        profile: LoopProfile {
            thresholds,
            ..LoopProfile::default()
        },
        ..state
    }
}

/// Asserts the generated ladder and [`route`] agree about `state`.
fn assert_ladder_parity(state: &LoopState) {
    let scope = expr_scope(state, LOOP_ID);
    let evaluated = expr::evaluate(&Value::String(ladder()), &scope);
    assert_ne!(evaluated, Value::Null, "ladder produced null for {state:?}");
    assert_eq!(
        evaluated.as_str(),
        Some(route(state).as_str()),
        "ladder and route disagree for {state:?}"
    );
}

/// Asserts the generated terminal condition and [`is_terminal`] agree.
fn assert_terminal_parity(state: &LoopState) {
    let scope = expr_scope(state, LOOP_ID);
    let evaluated = expr::evaluate(&Value::String(terminal_condition()), &scope);
    assert_ne!(
        evaluated,
        Value::Null,
        "terminal condition produced null for {state:?}"
    );
    assert_eq!(
        evaluated.as_bool(),
        Some(is_terminal(state)),
        "terminal condition and is_terminal disagree for {state:?}"
    );
}

/// Runs `body` once per value of `outer`, spreading the values over threads.
///
/// The sweep is tens of thousands of jq compilations; splitting the outermost
/// dimension keeps it a few seconds rather than a minute, and every case is
/// independent so there is nothing to synchronize but the counter.
fn in_parallel(outer: std::ops::RangeInclusive<u32>, body: &(impl Fn(u32) + Sync)) {
    std::thread::scope(|scope| {
        for value in outer {
            scope.spawn(move || body(value));
        }
    });
}

#[test]
fn the_ladder_agrees_with_route_on_every_combination() {
    let cases = AtomicUsize::new(0);
    for thresholds in threshold_sets() {
        in_parallel(upto(thresholds.blocked), &|blocked| {
            let mut swept = 0;
            for solved in [false, true] {
                for attempts in upto(thresholds.max_attempts) {
                    for unverified in upto(thresholds.unverified) {
                        for unproductive in upto(thresholds.stuck) {
                            for computational in upto(thresholds.computational) {
                                let state = LoopState {
                                    attempts,
                                    unproductive,
                                    blocked,
                                    computational,
                                    unverified,
                                    solved,
                                    ..LoopState::new("sweep")
                                };
                                assert_ladder_parity(&under(state, thresholds));
                                swept += 1;
                            }
                        }
                    }
                }
            }
            cases.fetch_add(swept, Ordering::Relaxed);
        });
    }

    // The whole input space of `route`, for every threshold set. The
    // expectation is computed from the same sets the sweep iterates rather than
    // written out, so adding a preset extends the sweep and its assertion
    // together instead of turning one into a stale number.
    let expected: usize = threshold_sets()
        .iter()
        .map(|thresholds| {
            span(thresholds.blocked)
                * 2
                * span(thresholds.max_attempts)
                * span(thresholds.unverified)
                * span(thresholds.stuck)
                * span(thresholds.computational)
        })
        .sum();
    assert_eq!(cases.into_inner(), expected);
}

#[test]
fn the_terminal_condition_agrees_with_is_terminal_on_every_combination() {
    let cases = AtomicUsize::new(0);
    for thresholds in threshold_sets() {
        in_parallel(upto(thresholds.blocked), &|blocked| {
            let mut swept = 0;
            for expired in [false, true] {
                for solved in [false, true] {
                    for restarts in upto(thresholds.max_restarts) {
                        for attempts in upto(thresholds.max_attempts) {
                            for unverified in upto(thresholds.unverified) {
                                let state = LoopState {
                                    attempts,
                                    blocked,
                                    unverified,
                                    restarts,
                                    solved,
                                    expired,
                                    ..LoopState::new("sweep")
                                };
                                assert_terminal_parity(&under(state, thresholds));
                                swept += 1;
                            }
                        }
                    }
                }
            }
            cases.fetch_add(swept, Ordering::Relaxed);
        });
    }

    let expected: usize = threshold_sets()
        .iter()
        .map(|thresholds| {
            span(thresholds.blocked)
                * 2
                * 2
                * span(thresholds.max_restarts)
                * span(thresholds.max_attempts)
                * span(thresholds.unverified)
        })
        .sum();
    assert_eq!(cases.into_inner(), expected);
}

#[test]
fn the_ladder_addresses_thresholds_rather_than_rendering_them() {
    // The guard this replaces asserted the opposite — that every threshold was
    // interpolated into the program. It is still the same class of failure
    // being guarded against, a second copy of a constant free to drift, and the
    // answer is now one address rather than one render.
    let program = ladder();

    for field in [
        "blocked",
        "max_attempts",
        "unverified",
        "stuck",
        "computational",
    ] {
        assert!(
            program.contains(&format!("$t | .{field}")),
            "{field} is not read out of the profile: {program}"
        );
    }
    assert!(program.contains(".profile.thresholds"), "{program}");
    for rendered in [">= 8", ">= 2", ">= 1", ">= 4", ">= 12"] {
        assert!(
            !program.contains(rendered),
            "{rendered} rendered: {program}"
        );
    }
}

#[test]
fn the_terminal_condition_addresses_thresholds_rather_than_rendering_them() {
    let program = terminal_condition();

    for field in ["max_restarts", "max_attempts", "blocked", "unverified"] {
        assert!(
            program.contains(&format!("$t | .{field}")),
            "{field} is not read out of the profile: {program}"
        );
    }
    for rendered in [">= 8", ">= 2", ">= 1", ">= 4", ">= 12"] {
        assert!(
            !program.contains(rendered),
            "{rendered} rendered: {program}"
        );
    }
}

#[test]
fn a_ladder_reads_the_thresholds_out_of_the_accumulator() {
    // Two states differing only in a threshold route differently through the
    // *same* program. This is the whole change, stated as one assertion.
    let mut state = LoopState::new("goal");
    state.unproductive = 2;

    let patient = under(
        state.clone(),
        Thresholds {
            stuck: 4,
            ..Thresholds::default()
        },
    );
    let impatient = under(
        state,
        Thresholds {
            stuck: 1,
            ..Thresholds::default()
        },
    );

    let program = Value::String(ladder());
    assert_eq!(
        expr::evaluate(&program, &expr_scope(&patient, LOOP_ID)).as_str(),
        Some("retry")
    );
    assert_eq!(
        expr::evaluate(&program, &expr_scope(&impatient, LOOP_ID)).as_str(),
        Some("diversify")
    );
}

#[test]
fn a_state_with_no_profile_routes_retry() {
    // A missing key is `null`, `null` sorts below every number in jq, and
    // `0 >= null` is *true* — so an unguarded read would fire the first rung
    // and route `blocked` on a state that simply had no profile. The sentinel
    // is what points the default at the cheap outcome instead.
    let evaluated = expr::evaluate(
        &Value::String(ladder()),
        &serde_json::json!({ "item": { "blocked": 0, "attempts": 0 } }),
    );
    assert_eq!(evaluated.as_str(), Some("retry"));

    let terminal = expr::evaluate(
        &Value::String(terminal_condition()),
        &serde_json::json!({ "item": { "blocked": 0, "attempts": 0 } }),
    );
    assert_eq!(terminal.as_bool(), Some(false));
}

#[test]
fn a_ladder_reads_the_accumulator_from_the_loop_head_state() {
    // The `until` position: the engine adds the post-fold accumulator as
    // `state`, and there is no `item`.
    let state = LoopState {
        blocked: 2,
        ..LoopState::new("goal")
    };
    let scope = serde_json::json!({ "state": serde_json::to_value(&state).unwrap() });

    let evaluated = expr::evaluate(&Value::String(ladder()), &scope);
    assert_eq!(evaluated.as_str(), Some("blocked"));
}

#[test]
fn a_ladder_reads_the_accumulator_from_the_previous_step() {
    // The downstream position: the accumulator arrives as the node's input.
    let state = LoopState {
        unverified: 2,
        ..LoopState::new("goal")
    };
    let scope = serde_json::json!({ "item": serde_json::to_value(&state).unwrap() });

    let evaluated = expr::evaluate(&Value::String(ladder()), &scope);
    assert_eq!(evaluated.as_str(), Some("reported"));
}

#[test]
fn an_empty_accumulator_still_routes() {
    // A loop whose accumulator has not been seeded yet must produce the cheap
    // route rather than null.
    let evaluated = expr::evaluate(&Value::String(ladder()), &serde_json::json!({ "item": {} }));
    assert_eq!(evaluated.as_str(), Some("retry"));
}

#[test]
fn retries_a_run_that_has_done_nothing_notable() {
    let state = LoopState::new("goal");
    assert_eq!(route(&state), Route::Retry);
}

#[test]
fn diversifies_after_two_unproductive_passes() {
    let state = LoopState {
        unproductive: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Diversify);
}

#[test]
fn diversifies_after_two_computational_passes() {
    let state = LoopState {
        computational: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Diversify);
}

#[test]
fn reports_an_answer_only_one_route_reached() {
    let state = LoopState {
        unverified: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Reported);
}

#[test]
fn solves_a_run_that_reached_an_answer() {
    let state = LoopState {
        solved: true,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Solved);
}

#[test]
fn solves_a_run_that_spent_its_attempts() {
    let state = LoopState {
        attempts: 8,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Solved);
}

#[test]
fn blocks_a_run_whose_machinery_kept_failing() {
    let state = LoopState {
        blocked: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Blocked);
}

#[test]
fn blocked_outranks_solved() {
    let state = LoopState {
        blocked: 2,
        solved: true,
        attempts: 8,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Blocked);
}

#[test]
fn reported_outranks_both_diversify_triggers() {
    let state = LoopState {
        unverified: 2,
        unproductive: 2,
        computational: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Reported);
}

#[test]
fn solved_outranks_reported() {
    let state = LoopState {
        solved: true,
        unverified: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(route(&state), Route::Solved);
}

#[test]
fn a_run_out_of_restarts_is_terminal_without_being_a_route() {
    let state = LoopState {
        restarts: 2,
        ..LoopState::new("goal")
    };

    assert_eq!(route(&state), Route::Retry);
    assert!(is_terminal(&state));
}

#[test]
fn a_run_with_time_left_and_no_verdict_is_not_terminal() {
    assert!(!is_terminal(&LoopState::new("goal")));
}

#[test]
fn evaluating_the_ladder_returns_the_same_route() {
    let state = LoopState {
        unproductive: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(
        super::evaluate_ladder(&state, LOOP_ID).unwrap(),
        Route::Diversify
    );
}

#[test]
fn evaluating_the_terminal_condition_returns_the_same_answer() {
    let state = LoopState {
        expired: true,
        ..LoopState::new("goal")
    };
    assert!(super::evaluate_terminal_condition(&state, LOOP_ID).unwrap());
}

#[test]
fn the_scope_offers_the_accumulator_at_every_address_the_engine_does() {
    let state = LoopState {
        passes: 4,
        ..LoopState::new("goal")
    };
    let scope = expr_scope(&state, LOOP_ID);

    assert_eq!(scope["item"]["passes"], 4);
    assert_eq!(scope["items"][0]["passes"], 4);
    assert_eq!(scope["state"]["passes"], 4);
    assert_eq!(scope["nodes"][LOOP_ID]["state"]["passes"], 4);
    assert_eq!(scope["nodes"][LOOP_ID]["iteration"], 4);
}

#[test]
fn route_names_round_trip_through_parse() {
    for expected in [
        Route::Blocked,
        Route::Solved,
        Route::Reported,
        Route::Diversify,
        Route::Retry,
    ] {
        assert_eq!(Route::parse(expected.as_str()), expected);
    }
}

#[test]
fn an_unreadable_route_falls_through_to_the_cheap_one() {
    for unreadable in ["", "divrsify", "SOLVE", "  ", "route"] {
        assert_eq!(Route::parse(unreadable), Route::Retry);
    }
}

#[test]
fn route_parsing_ignores_case_and_surrounding_space() {
    assert_eq!(Route::parse("  BLOCKED\n"), Route::Blocked);
}

#[test]
fn an_unreadable_route_never_ends_a_run() {
    assert!(!Route::parse("nonsense").is_terminal());
}

#[test]
fn judgement_names_round_trip_through_parse() {
    for expected in [Judgement::Proceed, Judgement::Steer, Judgement::Restart] {
        assert_eq!(Judgement::parse(expected.as_str()), expected);
    }
}

#[test]
fn an_unreadable_judgement_falls_through_to_proceed() {
    for unreadable in ["", "restrt", "PROCEE", "??"] {
        assert_eq!(Judgement::parse(unreadable), Judgement::Proceed);
    }
}

#[test]
fn judgement_parsing_ignores_case_and_surrounding_space() {
    assert_eq!(Judgement::parse(" Restart "), Judgement::Restart);
}

#[test]
fn the_wire_names_match_the_serde_representation() {
    for route in [
        Route::Blocked,
        Route::Solved,
        Route::Reported,
        Route::Diversify,
        Route::Retry,
    ] {
        assert_eq!(serde_json::to_value(route).unwrap(), route.as_str());
    }
    for judgement in [Judgement::Proceed, Judgement::Steer, Judgement::Restart] {
        assert_eq!(serde_json::to_value(judgement).unwrap(), judgement.as_str());
    }
}

#[test]
fn the_default_route_and_judgement_are_the_cheap_ones() {
    assert_eq!(Route::default(), Route::Retry);
    assert_eq!(Judgement::default(), Judgement::Proceed);
}

#[test]
fn the_default_thresholds_are_the_documented_bets() {
    let thresholds = Thresholds::default();
    assert_eq!(thresholds.max_attempts, 8);
    assert_eq!(thresholds.stuck, 2);
    assert_eq!(thresholds.blocked, 2);
    assert_eq!(thresholds.computational, 2);
    assert_eq!(thresholds.unverified, 2);
    assert_eq!(thresholds.max_restarts, 2);
    assert_eq!(thresholds.plan_interval, 3);
}

#[test]
fn thresholds_round_trip_through_their_wire_form() {
    let thresholds = Thresholds::default();
    let encoded = serde_json::to_value(thresholds).unwrap();
    assert_eq!(encoded["max_attempts"], 8);
    assert_eq!(
        serde_json::from_value::<Thresholds>(serde_json::json!({})).unwrap(),
        thresholds
    );
}

#[test]
fn planning_happens_on_every_interval() {
    let thresholds = Thresholds::default();
    assert!(!thresholds.plans_on(0));
    assert!(!thresholds.plans_on(2));
    assert!(thresholds.plans_on(3));
    assert!(thresholds.plans_on(6));
}

#[test]
fn a_zero_plan_interval_disables_planning_rather_than_dividing_by_zero() {
    let thresholds = Thresholds {
        plan_interval: 0,
        ..Thresholds::default()
    };
    assert!(!thresholds.plans_on(0));
    assert!(!thresholds.plans_on(7));
}

#[test]
fn autonomy_defaults_to_the_conservative_setting() {
    assert_eq!(Autonomy::default(), Autonomy::Report);
    assert_eq!(
        serde_json::to_value(Autonomy::Assisted).unwrap(),
        "assisted"
    );
    assert_eq!(
        serde_json::from_value::<Autonomy>(serde_json::json!("unattended")).unwrap(),
        Autonomy::Unattended
    );
}

#[test]
fn a_solved_run_that_banked_something_is_a_success() {
    let state = LoopState {
        solved: true,
        banked: 1,
        ..LoopState::new("goal")
    };
    assert_eq!(Outcome::classify(&state), Outcome::Success);
    assert_eq!(Outcome::success(&state).unwrap(), Outcome::Success);
}

#[test]
fn a_solved_run_that_banked_nothing_is_a_clean_no_op() {
    let state = LoopState {
        solved: true,
        ..LoopState::new("goal")
    };
    assert_eq!(Outcome::classify(&state), Outcome::CleanNoOp);
}

#[test]
fn an_expired_run_is_never_a_success() {
    let state = LoopState {
        solved: true,
        banked: 3,
        expired: true,
        ..LoopState::new("goal")
    };
    assert_eq!(Outcome::classify(&state), Outcome::Exhausted);
    assert_eq!(
        Outcome::success(&state).unwrap_err(),
        Error::UnearnedSuccess
    );
}

#[test]
fn a_run_out_of_attempts_is_never_a_success() {
    let state = LoopState {
        solved: true,
        banked: 3,
        attempts: 8,
        ..LoopState::new("goal")
    };
    assert_eq!(Outcome::classify(&state), Outcome::Exhausted);
    assert_eq!(
        Outcome::success(&state).unwrap_err(),
        Error::UnearnedSuccess
    );
}

#[test]
fn a_blocked_run_is_never_a_success() {
    let state = LoopState {
        solved: true,
        banked: 3,
        blocked: 2,
        ..LoopState::new("goal")
    };
    assert_eq!(Outcome::classify(&state), Outcome::Blocked);
    assert_eq!(
        Outcome::success(&state).unwrap_err(),
        Error::UnearnedSuccess
    );
}

#[test]
fn an_unsolved_run_with_budget_left_is_stalled() {
    assert_eq!(Outcome::classify(&LoopState::new("goal")), Outcome::Stalled);
}

#[test]
fn outcomes_have_a_snake_case_wire_form() {
    assert_eq!(
        serde_json::to_value(Outcome::CleanNoOp).unwrap(),
        "clean_no_op"
    );
    assert_eq!(
        serde_json::from_value::<Outcome>(serde_json::json!("exhausted")).unwrap(),
        Outcome::Exhausted
    );
}

#[test]
fn a_default_profile_carries_the_balanced_thresholds() {
    let profile = LoopProfile::default();
    assert_eq!(profile.revision, 0);
    assert_eq!(profile.thresholds, Thresholds::default());
    assert_eq!(profile.origin, crate::presets::Preset::Balanced);
    assert_eq!(LoopProfile::of(crate::presets::Preset::Balanced), profile);
}

#[test]
fn a_profile_takes_the_thresholds_of_the_preset_it_names() {
    for preset in crate::presets::Preset::ALL {
        let profile = LoopProfile::of(preset);
        assert_eq!(profile.thresholds, preset.thresholds());
        assert_eq!(profile.origin, preset);
        assert_eq!(profile.revision, 0);
    }
}

#[test]
fn the_profile_wire_form_is_pinned() {
    // The graph's jq addresses these names. A rename is a decode error at run
    // time rather than a compile error, which is what this pins.
    assert_eq!(
        serde_json::to_value(LoopProfile::of(crate::presets::Preset::Persistent)).unwrap(),
        serde_json::json!({
            "revision": 0,
            "thresholds": {
                "max_attempts": 12,
                "stuck": 4,
                "blocked": 2,
                "computational": 2,
                "unverified": 2,
                "max_restarts": 2,
                "plan_interval": 3,
            },
            "origin": "persistent",
            "caps": serde_json::to_value(crate::budget::Caps::default()).unwrap(),
            "muted": [],
            "history": [],
        })
    );
}

#[test]
fn a_profile_written_without_a_revision_deserializes() {
    // `serde(default)` at the container level: an accumulator written by a
    // revision that lacked a field still decodes, taking that field's default.
    let decoded: LoopProfile =
        serde_json::from_value(serde_json::json!({ "origin": "cautious" })).unwrap();
    assert_eq!(decoded.revision, 0);
    assert_eq!(decoded.thresholds, Thresholds::default());
    assert_eq!(decoded.origin, crate::presets::Preset::Cautious);

    let empty: LoopProfile = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, LoopProfile::default());
}

// --- amendments and bounds -------------------------------------------------

/// The change every test below starts from.
fn stuck_to(value: u32) -> Change {
    Change::Threshold {
        field: ThresholdField::Stuck,
        to: value,
    }
}

#[test]
fn a_threshold_field_reads_and_writes_the_field_it_names() {
    // The one place a field name and a struct field are joined by hand, so the
    // round trip is asserted rather than assumed. A `Stuck` that wrote
    // `blocked` would move a run's routing to a field nobody proposed.
    for (index, field) in ThresholdField::ALL.into_iter().enumerate() {
        let mut thresholds = Thresholds::default();
        let written = u32::try_from(index).unwrap() + 40;
        field.write(&mut thresholds, written);

        assert_eq!(field.read(&thresholds), written);
        for other in ThresholdField::ALL {
            if other != field {
                assert_eq!(
                    other.read(&thresholds),
                    other.read(&Thresholds::default()),
                    "{field} moved {other}",
                );
            }
        }
    }
}

#[test]
fn a_cap_field_reads_and_writes_the_field_it_names() {
    for field in CapField::ALL {
        let mut caps = crate::budget::Caps::default();
        field.write(&mut caps, 7);
        assert_eq!(field.read(&caps), 7);
    }
}

#[test]
fn the_amendment_wire_form_is_pinned() {
    let amendment = Amendment::new("tune", 3, stuck_to(3), "diversifying did not pay");

    assert_eq!(
        serde_json::to_value(&amendment).unwrap(),
        serde_json::json!({
            "proposer": "tune",
            "pass": 3,
            "change": { "change": "threshold", "field": "stuck", "to": 3 },
            "because": "diversifying did not pay",
        })
    );
    assert_eq!(
        serde_json::from_value::<Amendment>(serde_json::to_value(&amendment).unwrap()).unwrap(),
        amendment,
    );
}

#[test]
fn every_change_round_trips_and_renders() {
    let changes = [
        stuck_to(3),
        Change::Cap {
            field: CapField::MaxTokens,
            to: 1_000,
        },
        Change::MuteArm {
            arm: "judge".to_owned(),
        },
        Change::UnmuteArm {
            arm: "judge".to_owned(),
        },
    ];

    for change in changes {
        let encoded = serde_json::to_value(&change).unwrap();
        assert_eq!(serde_json::from_value::<Change>(encoded).unwrap(), change);
        assert!(!change.to_string().is_empty());
    }
}

#[test]
fn a_change_names_the_arm_it_moves_and_nothing_else() {
    assert_eq!(
        Change::MuteArm {
            arm: "judge".to_owned()
        }
        .arm(),
        Some("judge")
    );
    assert_eq!(stuck_to(3).arm(), None);
}

#[test]
fn a_range_holds_its_endpoints_and_nothing_past_them() {
    let range = Range::new(1, 4);
    assert!(range.holds(1));
    assert!(range.holds(4));
    assert!(!range.holds(0));
    assert!(!range.holds(5));
}

#[test]
fn a_change_outside_its_range_is_refused_rather_than_clamped() {
    let bounds = Bounds::none().threshold(ThresholdField::Stuck, Range::new(1, 4));

    assert!(bounds.check(&stuck_to(4)).is_ok());
    assert_eq!(
        bounds.check(&stuck_to(5)).unwrap_err(),
        Error::AmendmentOutOfBounds {
            field: "stuck".to_owned(),
            value: 5,
            low: 1,
            high: 4,
        },
    );
}

#[test]
fn a_field_with_no_bound_cannot_be_amended_at_all() {
    // The safe default, and the deliberate one: bounds written without thinking
    // about a field are bounds that do not let a tuner touch it.
    let bounds = Bounds::none();
    assert_eq!(
        bounds.check(&stuck_to(2)).unwrap_err(),
        Error::UnboundedAmendment {
            field: "stuck".to_owned(),
        },
    );
    assert_eq!(
        bounds
            .check(&Change::MuteArm {
                arm: "judge".to_owned()
            })
            .unwrap_err(),
        Error::UnboundedAmendment {
            field: "judge".to_owned(),
        },
    );
}

#[test]
fn narrowing_only_ever_tightens() {
    let preset = Bounds::none()
        .threshold(ThresholdField::Stuck, Range::new(1, 6))
        .threshold(ThresholdField::MaxAttempts, Range::new(4, 12))
        .cap(CapField::MaxTokens, Range::new(1, 1_000))
        .mutable("judge")
        .mutable("reflect")
        .amendments(4)
        .window(2);
    let deployment = Bounds::none()
        .threshold(ThresholdField::Stuck, Range::new(2, 3))
        .cap(CapField::MaxTokens, Range::new(1, 9_999))
        .mutable("judge")
        .amendments(99)
        .window(9);

    let narrowed = preset.narrow(&deployment);

    // Intersected where both spoke.
    assert_eq!(
        narrowed.thresholds[&ThresholdField::Stuck],
        Range::new(2, 3)
    );
    assert_eq!(narrowed.caps[&CapField::MaxTokens], Range::new(1, 1_000));
    // Dropped where only one did: silence is not permission.
    assert!(
        !narrowed
            .thresholds
            .contains_key(&ThresholdField::MaxAttempts)
    );
    assert_eq!(narrowed.mutable_arms.len(), 1);
    // A deployment cannot buy the run more amendments, or a shorter window
    // before an arm may be retired.
    assert_eq!(narrowed.max_amendments, 4);
    assert_eq!(narrowed.muting_window, 9);
}

#[test]
fn folding_an_amendment_bumps_the_revision_and_records_it() {
    let bounds = crate::presets::Preset::Balanced.bounds();
    let mut profile = LoopProfile::of(crate::presets::Preset::Balanced);

    let verdict = profile.fold(Amendment::new("tune", 1, stuck_to(3), "because"), &bounds);

    assert!(verdict.applied());
    assert_eq!(profile.thresholds.stuck, 3);
    assert_eq!(profile.revision, 1);
    assert_eq!(profile.applied(), 1);
    assert_eq!(profile.history.len(), 1);
    assert!(profile.history[0].to_string().contains("stuck := 3"));
}

#[test]
fn a_refused_amendment_leaves_the_profile_byte_identical() {
    let bounds = crate::presets::Preset::Balanced.bounds();
    let mut profile = LoopProfile::of(crate::presets::Preset::Balanced);
    let before = profile.clone();

    let verdict = profile.fold(Amendment::new("tune", 1, stuck_to(40), "because"), &bounds);

    assert!(!verdict.applied());
    assert_eq!(profile.thresholds, before.thresholds);
    assert_eq!(profile.revision, 0);
    assert_eq!(profile.applied(), 0);
    // Recorded, though. A refusal nobody can see is a broken tuner nobody can
    // see either.
    assert_eq!(profile.history.len(), 1);
    assert!(profile.history[0].to_string().contains("refused"));
}

#[test]
fn a_cap_amendment_that_would_contend_the_budget_is_refused_rather_than_applied() {
    // `MaxToolCalls` alone sits inside its declared range, but driving it down
    // to 1 while `max_model_calls` stays at its default of 60 leaves a `Caps`
    // `RunBudget::new` refuses — the tool cap can no longer be reached before
    // the model-call cap. A per-field range check alone would read this as
    // fine; folding must also ask whether the *combination* still budgets.
    let bounds = Bounds::none()
        .cap(CapField::MaxToolCalls, Range::new(1, 600))
        .amendments(1);
    let mut profile = LoopProfile::of(crate::presets::Preset::Balanced);
    let before = profile.clone();

    let verdict = profile.fold(
        Amendment::new(
            "tune",
            1,
            Change::Cap {
                field: CapField::MaxToolCalls,
                to: 1,
            },
            "because",
        ),
        &bounds,
    );

    assert!(!verdict.applied());
    assert_eq!(profile.caps, before.caps, "a refused cap amendment leaves caps untouched");
    assert!(
        profile.history[0]
            .to_string()
            .contains("reachable before"),
        "{}",
        profile.history[0]
    );
}

#[test]
fn a_run_at_its_amendment_budget_refuses_the_next_and_carries_on() {
    let bounds = Bounds::none()
        .threshold(ThresholdField::Stuck, Range::new(1, 9))
        .amendments(2);
    let mut profile = LoopProfile::of(crate::presets::Preset::Balanced);

    for value in [3, 4] {
        assert!(
            profile
                .fold(
                    Amendment::new("tune", 1, stuck_to(value), "because"),
                    &bounds
                )
                .applied()
        );
    }
    let third = profile.fold(Amendment::new("tune", 2, stuck_to(5), "because"), &bounds);

    assert!(!third.applied());
    assert_eq!(
        profile.thresholds.stuck, 4,
        "the profile stopped at the budget"
    );
    assert_eq!(profile.revision, 2);
    assert_eq!(profile.history.len(), 3);
}

#[test]
fn muting_moves_the_profile_and_unmuting_moves_it_back() {
    let bounds = Bounds::none().mutable("judge").amendments(2);
    let mut profile = LoopProfile::of(crate::presets::Preset::Balanced);

    profile.fold(
        Amendment::new(
            "tune",
            1,
            Change::MuteArm {
                arm: "judge".to_owned(),
            },
            "silent",
        ),
        &bounds,
    );
    assert!(profile.is_muted("judge"));

    profile.fold(
        Amendment::new(
            "tune",
            2,
            Change::UnmuteArm {
                arm: "judge".to_owned(),
            },
            "wanted again",
        ),
        &bounds,
    );
    assert!(!profile.is_muted("judge"));
}

#[test]
fn every_preset_states_its_bounds_and_none_of_them_permits_everything() {
    for preset in crate::presets::Preset::ALL {
        let bounds = preset.bounds();
        assert!(bounds.max_amendments > 0, "{preset} cannot revise itself");
        assert!(
            !bounds.thresholds.is_empty(),
            "{preset} bounds no threshold"
        );
        // No preset may tune its way out of the ceiling that stops a run: the
        // attempt cap can never be raised past the head's runaway backstop, or
        // the amendment folds, reads back as raised, and buys nothing.
        let backstop = u64::from(crate::budget::Caps::default().max_iterations);
        if let Some(range) = bounds.thresholds.get(&ThresholdField::MaxAttempts) {
            assert!(
                range.high <= backstop,
                "{preset} may amend past the backstop"
            );
        }
    }
}

#[test]
fn every_field_and_verdict_renders_the_name_it_is_addressed_by() {
    // The rendered names are what a reader of a run's events matches against
    // the field that moved, so they are asserted rather than assumed to follow
    // from the wire form.
    for field in ThresholdField::ALL {
        assert_eq!(field.to_string(), field.as_str());
        assert_eq!(
            serde_json::to_value(field).unwrap(),
            serde_json::Value::String(field.as_str().to_owned()),
        );
    }
    for field in CapField::ALL {
        assert_eq!(field.to_string(), field.as_str());
        assert_eq!(
            serde_json::to_value(field).unwrap(),
            serde_json::Value::String(field.as_str().to_owned()),
        );
    }

    assert_eq!(
        Change::Cap {
            field: CapField::MaxTokens,
            to: 10,
        }
        .to_string(),
        "max_tokens := 10",
    );
    assert_eq!(
        Change::UnmuteArm {
            arm: "judge".to_owned(),
        }
        .to_string(),
        "unmute judge",
    );

    assert!(crate::policy::Verdict::Applied.applied());
    assert!(
        !crate::policy::Verdict::Refused {
            reason: "no".to_owned(),
        }
        .applied()
    );
}
