//! Integration tests for the public crate surface.
//!
//! These tests link against the crate as a downstream consumer would: they can
//! only use what `src/lib.rs` re-exports. Treat them as the regression suite
//! for the crate's public contract — if a change breaks a test here, it is a
//! breaking change for users.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tinyloops::{Error, LoopProfile, LoopState, Preset, Route, greet, ladder, route};

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
