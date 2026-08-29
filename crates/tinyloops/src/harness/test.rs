//! Unit tests for the harness seam.
//!
//! Four of these exist because the corresponding failure was invisible while it
//! was happening, and they are why this module has assertions where it could
//! have had a convention:
//!
//! - a loop that sat 33 minutes awaiting one arm, which is why
//!   `no_method_both_starts_and_settles_work` reads the trait's own method list
//!   rather than trusting a reviewer to notice a `run_and_wait` being added;
//! - a note queue whose full state stalled the solve, which is why a drop is a
//!   returned value and a counted event rather than an `Err`;
//! - a delegation killed at its cap whose files were on disk and whose pass
//!   reported nothing, which is why every ending is an outcome;
//! - a role handed the run's whole budget, which is why declaring one without
//!   caps does not compile a default in its place.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tinyflows::caps::mock::mock_capabilities;

use super::*;

/// The source of the module root, read at compile time.
///
/// `no_method_both_starts_and_settles_work` scans it, so the absence of a
/// blocking delegation is asserted against the declaration itself rather than
/// against a list somebody has to remember to update.
const MOD_SOURCE: &str = include_str!("mod.rs");

/// The source of `types.rs`, read at compile time, for the same reason.
const TYPES_SOURCE: &str = include_str!("types.rs");

/// The field names a struct declares, read from the declaration itself.
///
/// "A role is four things and nothing else" is a claim about the type, so it is
/// asserted against the type rather than against a rendering of one value.
fn fields_of(source: &str, decl: &str) -> Vec<String> {
    source
        .split(decl)
        .nth(1)
        .expect("the struct is declared in this source")
        .split("\n}")
        .next()
        .expect("the declaration is brace-terminated")
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('/') && !line.starts_with('#'))
        .filter_map(|line| line.split(':').next())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Caps small enough to be obviously a role's rather than a run's.
fn role_caps() -> Caps {
    Caps {
        max_iterations: 1,
        max_model_calls: 4,
        max_tool_calls: 40,
        max_tokens: 100_000,
        run_timeout: Duration::from_secs(120),
        tool_timeout: Duration::from_secs(20),
        request_timeout: Duration::from_secs(10),
        max_retries: 1,
    }
}

/// A registry holding one `judge` role.
fn registry() -> RoleRegistry {
    let mut roles = RoleRegistry::new();
    roles
        .declare(
            "judge",
            "judge one attempt",
            RoleGrant::of(["read"]),
            Some(role_caps()),
            Tier::Standard,
        )
        .unwrap();
    roles
}

/// A harness whose `judge` role answers once.
fn answering() -> ScriptedDelegate {
    ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![Scripted::Answers {
            reply: "the attempt holds".to_owned(),
            artifacts: vec![Artifact::new("verdict.md", "the written verdict")],
        }],
    )
}

/// Counts drops, so "the drop was reported" is an assertion rather than a hope.
#[derive(Debug, Default)]
struct DropLog {
    seen: Mutex<Vec<(String, usize)>>,
}

impl DropLog {
    fn seen(&self) -> Vec<(String, usize)> {
        self.seen.lock().unwrap().clone()
    }
}

impl DropObserver for DropLog {
    fn dropped(&self, note: &Note, capacity: usize) {
        self.seen
            .lock()
            .unwrap()
            .push((note.body.clone(), capacity));
    }
}

#[test]
fn a_role_is_a_prompt_a_grant_a_budget_and_a_tier() {
    let role = Role::new(
        "judge one attempt",
        RoleGrant::of(["read", "search"]),
        role_caps(),
        Tier::Standard,
    )
    .unwrap();

    assert_eq!(role.prompt(), "judge one attempt");
    assert!(role.grant().allows("read"));
    assert_eq!(role.budget().caps().max_model_calls, 4);
    assert_eq!(role.tier(), Tier::Standard);

    // The fifth field is the one that must not exist, so the assertion reads
    // the declaration rather than a rendering of one value.
    assert_eq!(
        fields_of(TYPES_SOURCE, "pub struct Role {"),
        vec!["prompt", "grant", "budget", "tier"],
    );
}

#[test]
fn resolves_a_role_by_name() {
    let roles = registry();
    assert_eq!(roles.resolve("judge").unwrap().tier(), Tier::Standard);
    assert_eq!(roles.names().collect::<Vec<_>>(), vec!["judge"]);
    assert_eq!(roles.len(), 1);
    assert!(!roles.is_empty());
}

#[test]
fn an_unknown_role_name_is_an_error() {
    assert_eq!(
        registry().resolve("architect").unwrap_err(),
        Error::UnknownRole {
            role: "architect".to_owned()
        },
    );
}

#[test]
fn a_role_without_caps_is_a_construction_error() {
    let mut roles = RoleRegistry::new();
    assert_eq!(
        roles
            .declare(
                "summarizer",
                "four lines, no more",
                RoleGrant::none(),
                None,
                Tier::Small,
            )
            .unwrap_err(),
        Error::RoleWithoutCaps {
            role: "summarizer".to_owned()
        },
    );
    assert!(roles.is_empty());
}

#[test]
fn a_duplicate_role_name_is_refused() {
    let mut roles = registry();
    assert_eq!(
        roles
            .declare(
                "judge",
                "a second judge",
                RoleGrant::none(),
                Some(role_caps()),
                Tier::Deep,
            )
            .unwrap_err(),
        Error::DuplicateRole {
            role: "judge".to_owned()
        },
    );
    assert_eq!(roles.resolve("judge").unwrap().tier(), Tier::Standard);
}

#[test]
fn a_role_declared_with_an_illegal_budget_is_refused() {
    let mut roles = RoleRegistry::new();
    let caps = Caps {
        max_model_calls: 0,
        ..role_caps()
    };
    assert!(matches!(
        roles
            .declare(
                "broken",
                "prompt",
                RoleGrant::none(),
                Some(caps),
                Tier::Small
            )
            .unwrap_err(),
        Error::UnboundedCap { .. }
    ));
}

#[test]
fn a_grant_names_tools_and_never_enforces_them() {
    let grant = RoleGrant::of(["search", "read"]);
    assert_eq!(grant.names().collect::<Vec<_>>(), vec!["read", "search"]);
    assert_eq!(grant.len(), 2);
    assert!(!grant.is_empty());
    assert!(grant.allows("read"));
    assert!(!grant.allows("execute"));

    let empty = RoleGrant::none();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
}

#[test]
fn a_tier_round_trips_through_its_wire_name() {
    for tier in [Tier::Small, Tier::Standard, Tier::Deep] {
        assert_eq!(Tier::parse(tier.as_str()), Some(tier));
    }
    assert_eq!(Tier::parse("  DEEP  "), Some(Tier::Deep));
    assert_eq!(Tier::parse("deepest"), None);
}

#[test]
fn spawn_returns_a_ticket_without_waiting() {
    let delegate = answering();
    let ticket = delegate
        .spawn("judge", Brief::new("read the attempt"))
        .unwrap();
    assert_eq!(ticket.id(), "judge#0");
    // Nothing has been collected, and the loop is free to carry on.
    assert_eq!(delegate.peek(&ticket).unwrap(), Status::Ready);
}

#[test]
fn peek_reports_status_without_settling() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![Scripted::NeverCompletes {
            artifacts: Vec::new(),
        }],
    );
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();

    assert_eq!(delegate.peek(&ticket).unwrap(), Status::Running);
    assert_eq!(delegate.peek(&ticket).unwrap(), Status::Running);
    assert!(delegate.peek(&ticket).unwrap().is_outstanding());
    assert!(!Status::Settled.is_outstanding());
    assert!(!Status::Ready.is_outstanding());
    assert!(Status::Pending.is_outstanding());
}

#[tokio::test]
async fn a_settled_ticket_reports_settled() {
    let delegate = answering();
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();
    delegate.settle(&ticket).await.unwrap();
    assert_eq!(delegate.peek(&ticket).unwrap(), Status::Settled);
}

#[test]
fn steer_reaches_a_running_delegation() {
    let delegate = answering();
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();

    let posted = delegate
        .steer(&ticket, Note::new("loop", "the spec changed"))
        .unwrap();
    assert!(posted.is_accepted());

    let notes = delegate.notes(&ticket).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].body, "the spec changed");
    assert_eq!(notes[0].from, "loop");
}

#[test]
fn steering_past_the_mailbox_capacity_reports_the_drop() {
    let delegate = answering().with_mailbox_capacity(1);
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();

    assert!(
        delegate
            .steer(&ticket, Note::new("loop", "first"))
            .unwrap()
            .is_accepted()
    );
    let second = delegate
        .steer(&ticket, Note::new("loop", "second"))
        .unwrap();
    assert_eq!(second, Posted::Dropped(Note::new("loop", "second")));
    assert_eq!(delegate.dropped_notes(&ticket).unwrap(), 1);
}

#[test]
fn no_method_both_starts_and_settles_work() {
    let trait_body = MOD_SOURCE
        .split("pub trait Delegate")
        .nth(1)
        .expect("the Delegate trait is declared in this module")
        .split("\n}")
        .next()
        .expect("the trait declaration is brace-terminated");

    let methods: Vec<&str> = trait_body
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|rest| rest.split(['<', '(']).next())
        .collect();

    assert_eq!(methods, vec!["spawn", "peek", "steer", "settle"]);
    for forbidden in ["run_and_wait", "run", "execute", "delegate_blocking"] {
        assert!(
            !methods.contains(&forbidden),
            "{forbidden} both starts and settles work",
        );
    }
}

#[test]
fn capacity_is_declared_at_construction() {
    let mailbox = Mailbox::new(2);
    assert_eq!(mailbox.capacity(), 2);
    assert!(mailbox.is_empty());
    assert_eq!(mailbox.len(), 0);
}

#[test]
fn posting_at_capacity_drops_the_note_and_says_so() {
    let mailbox = Mailbox::new(1);
    assert_eq!(mailbox.post(Note::new("a", "kept")), Posted::Accepted);

    let dropped = mailbox.post(Note::new("a", "lost"));
    assert_eq!(dropped, Posted::Dropped(Note::new("a", "lost")));
    assert!(!dropped.is_accepted());
    assert_eq!(mailbox.len(), 1);
    assert_eq!(mailbox.drops(), 1);
}

#[test]
fn a_drop_emits_an_event() {
    let log = Arc::new(DropLog::default());
    let mailbox = Mailbox::observed(1, log.clone());

    mailbox.post(Note::new("a", "kept"));
    mailbox.post(Note::new("a", "lost"));

    assert_eq!(log.seen(), vec![("lost".to_owned(), 1)]);
}

#[test]
fn the_loop_takes_its_next_step_in_the_same_test() {
    let mailbox = Mailbox::new(1);
    mailbox.post(Note::new("a", "kept"));
    mailbox.post(Note::new("a", "lost"));

    // The drop did not block: this line runs, in this test, on this thread.
    let collected = mailbox.collect();
    assert_eq!(collected.len(), 1);
    assert!(mailbox.is_empty());
    // And the count survives the collect, so the loss is still reportable at
    // the end of the run rather than only at the moment it happened.
    assert_eq!(mailbox.drops(), 1);
    assert_eq!(mailbox.post(Note::new("a", "next")), Posted::Accepted);
}

#[tokio::test]
async fn a_timed_out_delegation_is_a_readable_outcome() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![Scripted::NeverCompletes {
            artifacts: vec![Artifact::new("partial.md", "half a verdict")],
        }],
    );
    let brief = Brief::new("read it").with_context("the attempt is attached");
    let ticket = delegate.spawn("judge", brief.clone()).unwrap();

    let outcome = delegate.settle(&ticket).await.unwrap();
    assert_eq!(outcome.brief, brief);
    assert_eq!(outcome.ending, Ending::TimedOut);
    assert_eq!(outcome.artifacts[0].path, "partial.md");
    assert!(outcome.reply.is_none());
    assert!(outcome.is_informative());
}

#[tokio::test]
async fn a_killed_delegation_that_wrote_an_artifact_is_salvaged() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![Scripted::Capped {
            artifacts: vec![Artifact::new("notes.md", "everything it wrote")],
        }],
    );
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();

    let outcome = delegate.settle(&ticket).await.unwrap();
    assert_eq!(outcome.ending, Ending::Capped);
    assert!(!outcome.ending.is_answered());
    assert_eq!(outcome.artifacts[0].description, "everything it wrote");
    assert!(outcome.is_informative());
}

#[test]
fn salvage_names_the_brief_and_keeps_the_files() {
    let outcome = salvage(
        Brief::new("survey"),
        vec![Artifact::new("survey.md", "partial")],
    );
    assert_eq!(outcome.brief.task, "survey");
    assert_eq!(outcome.ending.as_str(), "capped");
    assert!(outcome.reply.is_none());

    let silent = salvage(Brief::new("survey"), Vec::new());
    assert!(!silent.is_informative());
}

#[tokio::test]
async fn a_failed_delegation_is_not_an_error_return() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![Scripted::Fails {
            reason: "the sandbox died".to_owned(),
        }],
    );
    let ticket = delegate.spawn("judge", Brief::new("read it")).unwrap();

    let outcome = delegate
        .settle(&ticket)
        .await
        .expect("a failure is a result");
    assert_eq!(outcome.ending, Ending::Failed);
    assert_eq!(outcome.reply.as_deref(), Some("the sandbox died"));
}

#[test]
fn every_ending_has_a_wire_name() {
    for (ending, name) in [
        (Ending::Answered, "answered"),
        (Ending::TimedOut, "timed_out"),
        (Ending::Capped, "capped"),
        (Ending::Failed, "failed"),
    ] {
        assert_eq!(ending.as_str(), name);
    }
    assert!(Ending::Answered.is_answered());
}

#[tokio::test]
async fn scripted_outcomes_settle_in_the_order_the_script_declares() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
        "judge",
        vec![
            Scripted::Answers {
                reply: "first".to_owned(),
                artifacts: Vec::new(),
            },
            Scripted::Fails {
                reason: "second".to_owned(),
            },
        ],
    );

    let one = delegate.spawn("judge", Brief::new("a")).unwrap();
    let two = delegate.spawn("judge", Brief::new("b")).unwrap();
    assert_eq!(two.id(), "judge#1");

    // Settled out of order, and each ticket still gets its own scripted entry.
    let second = delegate.settle(&two).await.unwrap();
    let first = delegate.settle(&one).await.unwrap();
    assert_eq!(first.reply.as_deref(), Some("first"));
    assert_eq!(second.reply.as_deref(), Some("second"));
}

#[tokio::test]
async fn the_same_script_produces_the_same_events_on_every_run() {
    async fn run() -> Vec<DelegationOutcome> {
        let delegate = ScriptedDelegate::new(registry(), mock_capabilities()).scripting(
            "judge",
            vec![
                Scripted::Answers {
                    reply: "held".to_owned(),
                    artifacts: vec![Artifact::new("v.md", "verdict")],
                },
                Scripted::Capped {
                    artifacts: vec![Artifact::new("w.md", "partial")],
                },
            ],
        );
        let mut outcomes = Vec::new();
        for task in ["a", "b"] {
            let ticket = delegate.spawn("judge", Brief::new(task)).unwrap();
            outcomes.push(delegate.settle(&ticket).await.unwrap());
        }
        outcomes
    }

    assert_eq!(run().await, run().await);
}

#[tokio::test]
async fn settling_twice_returns_the_same_outcome() {
    let delegate = answering();
    let ticket = delegate.spawn("judge", Brief::new("a")).unwrap();
    let first = delegate.settle(&ticket).await.unwrap();
    let second = delegate.settle(&ticket).await.unwrap();
    assert_eq!(first, second);
}

#[test]
fn a_spawn_past_the_script_is_refused() {
    let delegate = answering();
    delegate.spawn("judge", Brief::new("a")).unwrap();
    assert_eq!(
        delegate.spawn("judge", Brief::new("b")).unwrap_err(),
        Error::SpawnRefused {
            role: "judge".to_owned(),
            reason: "the script declares no further outcomes".to_owned(),
        },
    );
}

#[test]
fn a_spawn_of_an_unscripted_role_is_refused() {
    let delegate = ScriptedDelegate::new(registry(), mock_capabilities());
    assert_eq!(
        delegate.spawn("judge", Brief::new("a")).unwrap_err(),
        Error::SpawnRefused {
            role: "judge".to_owned(),
            reason: "the script declares no outcomes for this role".to_owned(),
        },
    );
}

#[test]
fn a_spawn_of_an_undeclared_role_is_an_unknown_role() {
    let delegate = answering();
    assert_eq!(
        delegate.spawn("architect", Brief::new("a")).unwrap_err(),
        Error::UnknownRole {
            role: "architect".to_owned()
        },
    );
}

#[tokio::test]
async fn a_ticket_this_harness_did_not_issue_is_an_error() {
    let delegate = answering();
    let stranger = Ticket::new("elsewhere#0");
    let expected = Error::UnknownTicket {
        ticket: "elsewhere#0".to_owned(),
    };

    assert_eq!(delegate.peek(&stranger).unwrap_err(), expected);
    assert_eq!(
        delegate
            .steer(&stranger, Note::new("loop", "hi"))
            .unwrap_err(),
        expected,
    );
    assert_eq!(delegate.settle(&stranger).await.unwrap_err(), expected);
    assert_eq!(delegate.notes(&stranger).unwrap_err(), expected);
    assert_eq!(delegate.dropped_notes(&stranger).unwrap_err(), expected);
}

#[test]
fn it_opens_no_transport_of_its_own() {
    let delegate = answering();

    // The bundle is present, and the debug rendering names every field the
    // struct holds: a registry, a script, in-flight state, a mailbox bound, and
    // the capabilities. No client, no socket, no credential.
    assert!(delegate.capabilities().agent.is_none());
    let rendered = format!("{delegate:?}");
    assert!(rendered.contains("<Capabilities>"), "{rendered}");
    assert_eq!(
        fields_of(MOD_SOURCE, "pub struct ScriptedDelegate {"),
        vec!["roles", "caps", "script", "state", "mailbox_capacity"],
    );
}

#[test]
fn the_harness_reports_the_budget_a_role_runs_under() {
    let delegate = answering();
    assert_eq!(
        delegate.budget_for("judge").unwrap().caps().max_model_calls,
        4,
    );
    assert!(delegate.budget_for("architect").is_err());
    assert_eq!(delegate.roles().len(), 1);
    assert_eq!(DEFAULT_MAILBOX_CAPACITY, 4);
}

#[test]
fn a_brief_and_its_outcome_round_trip_as_json() {
    let outcome =
        DelegationOutcome::answered(Brief::new("read it").with_context("attached"), "it holds")
            .with_artifacts(vec![Artifact::new("v.md", "verdict")]);

    let json = serde_json::to_string(&outcome).unwrap();
    assert!(json.contains(r#""ending":"answered""#), "{json}");
    let back: DelegationOutcome = serde_json::from_str(&json).unwrap();
    assert_eq!(back, outcome);

    let ticket: Ticket = serde_json::from_str(r#""judge#0""#).unwrap();
    assert_eq!(ticket.id(), "judge#0");
    assert_eq!(
        serde_json::to_string(&RoleGrant::of(["read"])).unwrap(),
        r#"["read"]"#,
    );
    assert_eq!(serde_json::to_string(&Tier::Deep).unwrap(), r#""deep""#);
    assert_eq!(serde_json::to_string(&Status::Ready).unwrap(), r#""ready""#,);
}

#[test]
fn the_harness_failures_render_readably() {
    assert_eq!(
        Error::RoleWithoutCaps {
            role: "summarizer".to_owned()
        }
        .to_string(),
        "role summarizer was declared without caps",
    );
    assert_eq!(
        Error::DuplicateRole {
            role: "judge".to_owned()
        }
        .to_string(),
        "a role named judge is already declared",
    );
    assert_eq!(
        Error::UnknownRole {
            role: "architect".to_owned()
        }
        .to_string(),
        "no role named architect is declared",
    );
    assert_eq!(
        Error::UnknownTicket {
            ticket: "judge#0".to_owned()
        }
        .to_string(),
        "no delegation is held for ticket judge#0",
    );
    assert_eq!(
        Error::SpawnRefused {
            role: "judge".to_owned(),
            reason: "at capacity".to_owned(),
        }
        .to_string(),
        "spawn of judge refused: at capacity",
    );
}

#[test]
fn a_dropped_note_reaches_the_runs_own_event_stream() {
    // A drop that reaches only a counter is a designed loss nobody reading the
    // log can see, which is indistinguishable from a bug.
    #[derive(Debug, Default)]
    struct Collector(Mutex<Vec<crate::Event>>);

    impl crate::Sink for Collector {
        fn emit(&self, event: &crate::Event) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(event.clone());
            }
        }
    }

    let collector = Arc::new(Collector::default());
    let drops = Arc::new(SinkDrops::new(Arc::clone(&collector) as Arc<dyn crate::Sink>));
    drops.at_pass(4);

    let mailbox = Mailbox::observed(1, Arc::clone(&drops) as Arc<dyn DropObserver>);
    assert!(mailbox.post(Note::new("librarian", "kept")).is_accepted());
    assert!(!mailbox.post(Note::new("librarian", "lost")).is_accepted());

    let seen = collector.0.lock().expect("no test thread panicked");
    assert_eq!(seen.len(), 1, "only the dropped note is an event");
    assert_eq!(
        seen[0],
        crate::Event::NoteDropped {
            pass: 4,
            from: "librarian".to_owned(),
            capacity: 1,
        }
    );
    assert_eq!(crate::render(&seen[0]),
        "pass 4 note from librarian dropped, mailbox full at 1");
}
