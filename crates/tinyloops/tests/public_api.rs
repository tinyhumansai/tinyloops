//! Integration tests for the public crate surface.
//!
//! These tests link against the crate as a downstream consumer would: they can
//! only use what `src/lib.rs` re-exports. Treat them as the regression suite
//! for the crate's public contract — if a change breaks a test here, it is a
//! breaking change for users.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use tinyloops::{
    Amendment, Bounds, Change, DelegateSet, Error, FixedPlan, Inline, LineSink, LoopProfile,
    LoopState, Preset, Range, Recorder, Route, Scripted, ThresholdField, greet, ladder, route,
    tuned_research_loop,
};

#[test]
fn greeting_is_available_to_consumers() {
    assert_eq!(greet("Rust").unwrap(), "Hello, Rust!");
}

#[test]
fn errors_are_available_to_consumers() {
    assert_eq!(greet("").unwrap_err(), Error::EmptyName);
}

#[test]
fn a_run_carries_the_profile_it_routes_on() {
    // The whole public shape of the addressing change, from a consumer's side:
    // a profile is chosen once, rides in the accumulator, and is the only thing
    // `route` reads its thresholds from.
    let mut state = LoopState::with_profile("goal", LoopProfile::of(Preset::Persistent));
    state.unproductive = 2;

    assert_eq!(state.profile.origin, Preset::Persistent);
    assert_eq!(route(&state), Route::Retry);

    state.profile.thresholds.stuck = 1;
    assert_eq!(route(&state), Route::Diversify);
}

#[test]
fn the_ladder_a_consumer_reads_holds_no_threshold() {
    let program = ladder();
    assert!(program.contains(".profile.thresholds"));
    assert!(!program.contains(">= 8"));
}

#[test]
fn a_run_may_revise_itself_only_within_its_presets_bounds() {
    // The public shape of the whole adaptation surface: a preset states its
    // room, a proposal is folded or refused whole, and either way the run's own
    // record says what happened.
    let bounds = Preset::Balanced.bounds();
    let mut profile = LoopProfile::of(Preset::Balanced);

    let allowed = Amendment::new(
        "tune",
        1,
        Change::Threshold {
            field: ThresholdField::Stuck,
            to: 3,
        },
        "diversifying did not pay",
    );
    assert!(profile.fold(allowed, &bounds).applied());
    assert_eq!(profile.thresholds.stuck, 3);
    assert_eq!(profile.revision, 1);

    let absurd = Amendment::new(
        "tune",
        2,
        Change::Threshold {
            field: ThresholdField::Stuck,
            to: 40,
        },
        "patience solves everything",
    );
    assert!(!profile.fold(absurd, &bounds).applied());
    assert_eq!(profile.thresholds.stuck, 3, "a refusal changes nothing");
    assert_eq!(profile.history.len(), 2, "and is still recorded");
}

#[test]
fn a_deployment_can_narrow_a_preset_and_cannot_widen_one() {
    let narrowed = Preset::Balanced
        .bounds()
        .narrow(&Bounds::none().threshold(ThresholdField::Stuck, Range::new(2, 2)));

    assert!(
        narrowed
            .check(&Change::Threshold {
                field: ThresholdField::Stuck,
                to: 4,
            })
            .is_err(),
        "the deployment's tighter ceiling holds",
    );
    assert!(narrowed.max_amendments <= Preset::Balanced.bounds().max_amendments);
}

#[test]
fn a_tuned_loop_is_assemblable_from_the_public_surface_alone() {
    let delegates = DelegateSet::of(["prover"]);
    let assembled = tuned_research_loop(
        "bound the error term",
        Preset::Balanced,
        delegates.clone(),
        Arc::new(FixedPlan::of([(
            "bound",
            "bound the error term",
            "a proved bound",
        )])),
        Arc::new(Inline::of(
            delegates,
            [(
                "prover".to_owned(),
                vec![Scripted::Fails {
                    reason: "the sandbox would not start".to_owned(),
                }],
            )],
        )),
    )
    .expect("the tuned preset assembles");

    let driven = assembled
        .drive(&Recorder::new(
            "run",
            Arc::new(LineSink::new(std::io::sink())),
        ))
        .expect("it drives");

    assert_eq!(driven.profile, driven.state.profile);
    assert_eq!(driven.profile.origin, Preset::Balanced);
}
