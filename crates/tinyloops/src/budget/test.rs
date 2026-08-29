//! Unit tests for the run budget, its bounds, and its two meters.
//!
//! Three things are pinned here, and each one is pinned because its failure is
//! silent:
//!
//! - the **relationships between the caps**, because a configuration where the
//!   wrong cap trips first still runs, still stops, and still looks fine — it
//!   simply loses the report when it ends;
//! - the **per-role narrowing**, because a role handed the run's budget exceeds
//!   no cap while spending far more than its work is worth;
//! - the **two meters**, because a run that counts only raw compute cannot tell
//!   ten productive passes from ten that learned nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::*;
use crate::Error;

/// Caps that satisfy every rule, as a base for tests that break exactly one.
fn valid() -> Caps {
    Caps::default()
}

#[test]
fn the_default_caps_are_a_legal_configuration() {
    // `RunBudget::default` builds the struct directly rather than through the
    // validating constructor, so this is the test that keeps the default from
    // drifting into something no caller could have constructed.
    assert_eq!(RunBudget::new(Caps::default()).unwrap(), RunBudget::default());
}

#[test]
fn rejects_a_zero_iteration_cap() {
    let caps = Caps {
        max_iterations: 0,
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::UnboundedCap {
            bound: Bound::Iterations
        },
    );
}

#[test]
fn rejects_a_zero_token_cap() {
    let caps = Caps {
        max_tokens: 0,
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::UnboundedCap {
            bound: Bound::Tokens
        },
    );
}

#[test]
fn rejects_a_zero_run_clock() {
    let caps = Caps {
        run_timeout: Duration::ZERO,
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::UnboundedCap {
            bound: Bound::RunClock
        },
    );
}

#[test]
fn rejects_a_tool_timeout_that_outlives_the_run() {
    let caps = Caps {
        tool_timeout: Duration::from_secs(60 * 60),
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::NestedTimeout {
            inner: Bound::ToolTimeout,
            outer: Bound::RunClock,
        },
    );
}

#[test]
fn rejects_a_tool_timeout_equal_to_the_run_clock() {
    // Equal timeouts race, and the loser decides whether the run keeps its
    // report — so "strictly shorter" is the rule, not "no longer than".
    let caps = Caps {
        tool_timeout: valid().run_timeout,
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::NestedTimeout {
            inner: Bound::ToolTimeout,
            outer: Bound::RunClock,
        },
    );
}

#[test]
fn rejects_a_request_timeout_that_outlives_its_tool_call() {
    let caps = Caps {
        request_timeout: Duration::from_secs(300),
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::NestedTimeout {
            inner: Bound::RequestTimeout,
            outer: Bound::ToolTimeout,
        },
    );
}

#[test]
fn rejects_a_tool_cap_the_run_can_reach_before_its_model_cap() {
    let caps = Caps {
        max_model_calls: 60,
        max_tool_calls: 60,
        ..valid()
    };
    assert_eq!(
        RunBudget::new(caps).unwrap_err(),
        Error::ContendedCaps {
            reachable: Bound::ToolCalls,
            shadowed: Bound::ModelCalls,
        },
    );
}

#[test]
fn accepts_a_tool_cap_exactly_at_the_reachability_floor() {
    let caps = Caps {
        max_model_calls: 10,
        max_tool_calls: 10 * TOOL_CALLS_PER_MODEL_CALL,
        ..valid()
    };
    assert!(RunBudget::new(caps).is_ok());
}

#[test]
fn every_public_constructor_leaves_the_model_call_cap_reachable() {
    let run = RunBudget::default();
    let budgets = [
        run,
        RunBudget::new(Caps::default()).unwrap(),
        run.judging().unwrap(),
        run.housekeeping().unwrap(),
    ];

    for budget in budgets {
        let caps = budget.caps();
        assert_eq!(budget.reachable(), Bound::ModelCalls);
        assert!(budget.reachable().is_graceful());
        assert!(
            caps.max_tool_calls >= caps.max_model_calls * TOOL_CALLS_PER_MODEL_CALL,
            "the tool cap must sit above what the model cap can reach",
        );
        assert!(caps.tool_timeout < caps.run_timeout);
        assert!(caps.request_timeout < caps.tool_timeout);
    }
}

#[test]
fn a_judging_budget_is_narrower_than_the_run_it_serves() {
    let run = RunBudget::default();
    let judging = run.judging().unwrap().caps();

    assert!(judging.max_model_calls < run.caps().max_model_calls);
    assert!(judging.max_tokens < run.caps().max_tokens);
    assert!(judging.run_timeout < run.caps().run_timeout);
}

#[test]
fn a_housekeeping_budget_is_narrower_than_a_judging_one() {
    let run = RunBudget::default();
    let judging = run.judging().unwrap().caps();
    let housekeeping = run.housekeeping().unwrap().caps();

    assert!(housekeeping.max_model_calls < judging.max_model_calls);
    assert!(housekeeping.max_tokens < judging.max_tokens);
}

#[test]
fn narrowing_clamps_every_cap_to_the_parent() {
    // A role asking for more than the run it belongs to gets the run's number,
    // never its own: a child budget that could exceed its parent is not a
    // budget.
    let run = RunBudget::default();
    let greedy = Caps {
        max_iterations: 1_000,
        max_model_calls: 1_000,
        max_tool_calls: 100_000,
        max_tokens: 999_999_999,
        run_timeout: Duration::from_secs(60 * 60 * 24),
        tool_timeout: Duration::from_secs(60 * 60),
        request_timeout: Duration::from_secs(60 * 30),
        max_retries: 99,
    };

    assert_eq!(run.narrow(greedy).unwrap(), run);
}

#[test]
fn narrowing_reports_a_combination_it_cannot_satisfy() {
    // Clamping can leave caps that no longer satisfy the reachability rule.
    // That is reported rather than quietly repaired: silently widening the
    // tool cap would hand back a budget the caller did not ask for.
    let run = RunBudget::default();
    let contended = Caps {
        max_model_calls: 8,
        max_tool_calls: 8,
        ..Caps::default()
    };

    assert_eq!(
        run.narrow(contended).unwrap_err(),
        Error::ContendedCaps {
            reachable: Bound::ToolCalls,
            shadowed: Bound::ModelCalls,
        },
    );
}

#[test]
fn reports_no_bound_before_anything_is_spent() {
    assert_eq!(RunBudget::default().tripped(&Meter::default()), None);
}

#[test]
fn trips_on_iterations_when_the_loop_makes_no_progress() {
    // A loop that spins fast: no clock spent, no calls made, only passes.
    let budget = RunBudget::default();
    let mut meter = Meter::default();
    for _ in 0..budget.caps().max_iterations {
        meter.pass(false);
    }

    assert_eq!(budget.tripped(&meter), Some(Bound::Iterations));
}

#[test]
fn trips_on_the_run_clock_when_the_passes_are_slow() {
    // The companion to the test above, and deliberately not relying on its
    // bound: one pass, all the time.
    let budget = RunBudget::default();
    let mut meter = Meter::default();
    meter.pass(true);
    meter.advance(budget.caps().run_timeout);

    assert_eq!(budget.tripped(&meter), Some(Bound::RunClock));
}

#[test]
fn trips_on_the_model_call_cap() {
    let budget = RunBudget::default();
    let mut meter = Meter::default();
    for _ in 0..budget.caps().max_model_calls {
        meter.model_call(1);
    }

    let bound = budget.tripped(&meter).unwrap();
    assert_eq!(bound, Bound::ModelCalls);
    assert!(bound.is_graceful());
}

#[test]
fn trips_on_the_token_cap() {
    let budget = RunBudget::default();
    let mut meter = Meter::default();
    meter.model_call(budget.caps().max_tokens);

    assert_eq!(budget.tripped(&meter), Some(Bound::Tokens));
}

#[test]
fn the_tool_cap_still_reports_itself_if_it_is_somehow_reached() {
    // Unreachable through model calls by construction, so this drives it
    // directly: the backstop must be able to name itself rather than let a run
    // end with nothing.
    let budget = RunBudget::default();
    let mut meter = Meter::default();
    for _ in 0..budget.caps().max_tool_calls {
        meter.tool_call();
    }

    assert_eq!(budget.tripped(&meter), Some(Bound::ToolCalls));
}

#[test]
fn ten_signalling_passes_and_ten_inert_ones_share_a_raw_total() {
    let mut signalling = Meter::default();
    let mut inert = Meter::default();
    for _ in 0..10 {
        signalling.pass(true);
        inert.pass(false);
    }

    assert_eq!(signalling.iterations(), inert.iterations());
    assert_eq!(signalling.effective_passes(), 10);
    assert_eq!(signalling.inert_passes(), 0);
    assert_eq!(inert.effective_passes(), 0);
    assert_eq!(inert.inert_passes(), 10);
    assert_eq!(signalling.effective_ratio(), Some(1.0));
    assert_eq!(inert.effective_ratio(), Some(0.0));
}

#[test]
fn there_is_no_effective_ratio_before_the_first_pass() {
    // Not zero: "no passes yet" and "no pass learned anything" are opposite
    // situations, and a stopping rule that confuses them stops at pass zero.
    assert_eq!(Meter::default().effective_ratio(), None);
}

#[test]
fn the_meters_saturate_rather_than_wrapping() {
    let mut meter = Meter::default();
    meter.model_call(u64::MAX);
    meter.model_call(u64::MAX);
    meter.advance(Duration::MAX);
    meter.advance(Duration::MAX);

    assert_eq!(meter.tokens(), u64::MAX);
    assert_eq!(meter.elapsed(), Duration::MAX);
    assert_eq!(meter.model_calls(), 2);
}

#[test]
fn bound_names_round_trip() {
    for bound in [
        Bound::Iterations,
        Bound::RunClock,
        Bound::ModelCalls,
        Bound::ToolCalls,
        Bound::Tokens,
        Bound::ToolTimeout,
        Bound::RequestTimeout,
    ] {
        assert_eq!(Bound::parse(bound.as_str()), Some(bound));
    }
}

#[test]
fn an_unknown_bound_name_is_not_guessed_at() {
    assert_eq!(Bound::parse("model calls"), None);
    assert_eq!(Bound::parse(""), None);
    assert_eq!(Bound::parse("wall_clock"), None);
}

#[test]
fn only_the_bounds_that_keep_their_report_are_graceful() {
    assert!(Bound::ModelCalls.is_graceful());
    assert!(Bound::Iterations.is_graceful());
    assert!(Bound::Tokens.is_graceful());
    assert!(Bound::ToolTimeout.is_graceful());
    assert!(!Bound::RunClock.is_graceful());
    assert!(!Bound::ToolCalls.is_graceful());
    assert!(!Bound::RequestTimeout.is_graceful());
}

#[test]
fn caps_pin_their_wire_form() {
    // `Caps` is what a host sends to configure a run, so the field names are a
    // wire format: a rename is a decode error at runtime rather than a compile
    // error here.
    let caps = Caps {
        max_iterations: 3,
        max_model_calls: 5,
        max_tool_calls: 40,
        max_tokens: 1_000,
        run_timeout: Duration::from_secs(90),
        tool_timeout: Duration::from_secs(30),
        request_timeout: Duration::from_secs(10),
        max_retries: 2,
    };

    assert_eq!(
        serde_json::to_value(caps).unwrap(),
        json!({
            "max_iterations": 3,
            "max_model_calls": 5,
            "max_tool_calls": 40,
            "max_tokens": 1000,
            "run_timeout": { "secs": 90, "nanos": 0 },
            "tool_timeout": { "secs": 30, "nanos": 0 },
            "request_timeout": { "secs": 10, "nanos": 0 },
            "max_retries": 2,
        }),
    );
    assert_eq!(
        serde_json::from_value::<Caps>(serde_json::to_value(caps).unwrap()).unwrap(),
        caps,
    );
}

#[test]
fn a_bound_names_itself_in_the_error_it_causes() {
    let error = RunBudget::new(Caps {
        max_model_calls: 0,
        ..valid()
    })
    .unwrap_err();

    assert_eq!(error.to_string(), "budget cap model_calls must not be zero");
}
