//! Unit tests for the crate-wide error type.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn renders_a_human_readable_message() {
    assert_eq!(Error::EmptyName.to_string(), "name must not be empty");
}

#[test]
fn is_a_standard_error() {
    fn assert_error<E: std::error::Error>(_: &E) {}

    assert_error(&Error::EmptyName);
}

#[test]
fn the_amendment_refusals_render_the_messages_a_reader_will_see() {
    // These two strings are what a refusal carries into the run's record and
    // its report, so they are read by people rather than matched by code.
    assert_eq!(
        Error::UnboundedAmendment {
            field: "max_tokens".to_owned(),
        }
        .to_string(),
        "nothing bounds max_tokens, so it cannot be amended",
    );
    assert_eq!(
        Error::AmendmentOutOfBounds {
            field: "stuck".to_owned(),
            value: 9,
            low: 1,
            high: 4,
        }
        .to_string(),
        "stuck may be 1..=4, not 9",
    );
    assert_eq!(
        Error::AmbiguousTuning {
            first: "tune",
            second: "also_tune",
        }
        .to_string(),
        "both tune and also_tune may tune the loop",
    );
}
