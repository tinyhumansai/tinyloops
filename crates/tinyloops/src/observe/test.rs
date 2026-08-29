//! Unit tests for the loop's event stream, its sinks, and the report it ends
//! with.
//!
//! Four of these are regression tests for failures that were invisible while
//! they were happening, and they are the reason the module has assertions where
//! it could have had conventions:
//!
//! - a step that entered and never announced finishing, which cost 62 minutes
//!   of a production run that nobody could see into;
//! - a prompt reaching a log because capture defaulted the other way;
//! - a per-role view that was missing the run's spine, so the reader had to be
//!   on the right tab to see the run change course;
//! - a report that answered "did it pass" with one boolean, which is precisely
//!   the number that inverts across repeats.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use super::*;
use crate::policy::{Judgement, Outcome, Route};
use crate::state::Delta;

/// A credential-shaped fixture, so a leak is a substring search rather than a
/// judgement call.
const SECRET: &str = "sk-live-do-not-log-this";

/// A [`Sink`] that keeps every event it receives.
#[derive(Debug, Default)]
struct Collector {
    events: Mutex<Vec<Event>>,
}

impl Collector {
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl Sink for Collector {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// A writer that keeps what was written, so a line renderer can be read back.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A recorder over a collector, plus the collector.
fn recorder(capture: Capture) -> (Recorder, Arc<Collector>) {
    let collector = Arc::new(Collector::default());
    let recorder = Recorder::with_capture("loop", collector.clone(), capture);
    (recorder, collector)
}

/// A model call carrying a secret in both payload fields.
fn call_with_secret() -> ModelCall {
    let mut call = ModelCall::new("acme", "acme-small", "judge");
    call.prompt_tokens = 100;
    call.cached_tokens = 25;
    call.output_tokens = 10;
    call.cache_hit_rate = ModelCall::hit_rate_from_tokens(100, 25);
    call.cost = Some(0.02);
    call.prompt = Some(format!("use {SECRET} to authenticate"));
    call.completion = Some(format!("done, {SECRET}"));
    call
}

/// Everything in the journal, as one JSON string to scan.
fn serialized(recorder: &Recorder) -> String {
    serde_json::to_string(&recorder.journal()).unwrap()
}

#[test]
fn with_capture_off_no_prompt_text_reaches_the_journal_or_a_sink() {
    // The default, and the one that matters: observability that defaults to
    // recording prompts is a secret leak with a dashboard attached.
    let (recorder, collector) = recorder(Capture::default());
    assert_eq!(recorder.capture(), Capture::default());

    recorder.model_call(1, call_with_secret());
    let mut tool = ToolCall::new("shell", "solver");
    tool.arguments = Some(format!("curl -H 'auth: {SECRET}'"));
    tool.output = Some(SECRET.to_string());
    recorder.tool_call(1, tool);

    assert!(!serialized(&recorder).contains(SECRET));
    let emitted = serde_json::to_string(&collector.events()).unwrap();
    assert!(!emitted.contains(SECRET));
    // The numbers survive; only the text is gone.
    assert_eq!(recorder.accounting().run.prompt_tokens(), 100);
}

#[test]
fn with_capture_on_a_redacting_sink_keeps_the_secret_out_of_the_sink() {
    let collector = Arc::new(Collector::default());
    let redacting = Arc::new(RedactingSink::new(
        collector.clone(),
        vec![SECRET.to_string()],
    ));
    let recorder = Recorder::with_capture("loop", redacting, Capture::all());

    recorder.model_call(1, call_with_secret());

    let emitted = serde_json::to_string(&collector.events()).unwrap();
    assert!(!emitted.contains(SECRET));
    assert!(emitted.contains("[redacted]"));
    // Capture was asked for, so the recorder's own journal still holds the
    // text: redaction sits between capture and the sink, not before capture.
    assert!(serialized(&recorder).contains(SECRET));
}

#[test]
fn an_empty_secret_is_ignored_rather_than_masking_everything() {
    let collector = Arc::new(Collector::default());
    let redacting = RedactingSink::new(collector.clone(), vec![String::new()]);
    redacting.emit(&Event::PassStarted { pass: 1 });

    assert_eq!(collector.events(), vec![Event::PassStarted { pass: 1 }]);
    assert_eq!(redacting.drops(), 0);
}

#[test]
fn a_step_that_never_announces_finishing_is_reported() {
    // The 62-minute silent gap, as a value a test can fail on.
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::PassStarted { pass: 1 });
    recorder.record(Event::StepEntered {
        pass: 1,
        step: "solve".to_string(),
    });

    assert_eq!(
        recorder.unpaired(),
        vec![Unpaired {
            pass: 1,
            name: "solve".to_string(),
            unit: Unit::Step,
            entered: true,
        }],
    );
}

#[test]
fn a_step_that_finishes_without_having_entered_is_reported() {
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::StepFinished {
        pass: 1,
        step: "solve".to_string(),
        duration: Duration::from_millis(5),
    });

    assert_eq!(
        recorder.unpaired(),
        vec![Unpaired {
            pass: 1,
            name: "solve".to_string(),
            unit: Unit::Step,
            entered: false,
        }],
    );
}

#[test]
fn steps_and_arms_that_pair_up_report_nothing() {
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::PassStarted { pass: 1 });
    for name in ["solve", "verify"] {
        recorder.record(Event::StepEntered {
            pass: 1,
            step: name.to_string(),
        });
        recorder.record(Event::StepFinished {
            pass: 1,
            step: name.to_string(),
            duration: Duration::from_millis(3),
        });
    }
    recorder.record(Event::ArmStarted {
        pass: 1,
        arm: "wide".to_string(),
    });
    recorder.record(Event::ArmFinished {
        pass: 1,
        arm: "wide".to_string(),
        duration: Duration::from_millis(9),
    });

    assert!(recorder.unpaired().is_empty());
}

#[test]
fn the_same_step_name_in_two_passes_pairs_within_its_own_pass() {
    let (recorder, _) = recorder(Capture::default());
    for pass in 1..=2 {
        recorder.record(Event::StepEntered {
            pass,
            step: "solve".to_string(),
        });
    }
    recorder.record(Event::StepFinished {
        pass: 1,
        step: "solve".to_string(),
        duration: Duration::from_millis(1),
    });

    assert_eq!(
        recorder.unpaired(),
        vec![Unpaired {
            pass: 2,
            name: "solve".to_string(),
            unit: Unit::Step,
            entered: true,
        }],
    );
}

#[test]
fn a_view_filtered_to_one_role_still_contains_the_runs_spine() {
    // Nobody should have to be on the right tab to see the run change course.
    let (loop_view, _) = recorder(Capture::default());
    let judge = loop_view.child("judge");

    loop_view.record(Event::PassStarted { pass: 1 });
    judge.record(Event::StepEntered {
        pass: 1,
        step: "read-report".to_string(),
    });
    judge.record(Event::Judged {
        pass: 1,
        judgement: Judgement::Steer,
        score: 6,
    });
    loop_view.record(Event::Routed {
        pass: 1,
        route: Route::Diversify,
        reason: "two unproductive passes".to_string(),
    });
    loop_view.record(Event::BoundTripped {
        pass: 1,
        bound: crate::Bound::ModelCalls,
    });
    loop_view.record(Event::LoopFinished {
        pass: 1,
        outcome: Outcome::Stalled,
    });

    let view = loop_view.view("judge");
    let kinds: Vec<&str> = view.iter().map(|entry| entry.event.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            "pass_started",
            "step_entered",
            "judged",
            "routed",
            "bound_tripped",
            "loop_finished",
        ],
    );
    // The one non-spine entry in the view is the judge's own.
    assert!(
        view.iter()
            .all(|entry| entry.who == "judge" || entry.event.is_spine())
    );
}

#[test]
fn a_view_filtered_to_a_label_nothing_used_is_the_spine_alone() {
    let (loop_view, _) = recorder(Capture::default());
    loop_view.record(Event::PassStarted { pass: 1 });
    loop_view.record(Event::StepEntered {
        pass: 1,
        step: "solve".to_string(),
    });

    let view = loop_view.view("nobody");
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].event.kind(), "pass_started");
}

#[test]
fn a_child_shares_the_journal_and_the_counters() {
    let (loop_view, _) = recorder(Capture::default());
    let judge = loop_view.child("judge");
    assert_eq!(judge.who(), "judge");

    loop_view.record(Event::PassStarted { pass: 3 });
    judge.model_call(3, ModelCall::new("acme", "acme-small", "judge"));

    // One journal, in order, each entry labelled with who produced it.
    let journal = loop_view.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].who, "loop");
    assert_eq!(journal[1].who, "judge");
    assert_eq!(journal, judge.journal());
    // And the parent's counters include the child's.
    assert_eq!(loop_view.accounting().run.calls(), 1);
    // A child also sees the pass the parent announced.
    assert_eq!(judge.current_pass(), 3);
}

#[test]
fn accounting_names_the_model_that_actually_answered() {
    // Under a fallback ladder the route varies per call: the run must account
    // for the model that answered, not the one that was configured.
    let (recorder, _) = recorder(Capture::default());
    let mut fallback = ModelCall::new("acme", "acme-large-fallback", "solver");
    fallback.prompt_tokens = 10;
    fallback.cost = Some(0.5);
    recorder.model_call(1, fallback);

    let accounting = recorder.accounting();
    assert!(accounting.per_model.contains_key("acme-large-fallback"));
    assert_eq!(accounting.per_role["solver"].calls(), 1);
    assert_eq!(accounting.run.cost(), Some(0.5));
}

#[test]
fn a_call_the_provider_did_not_price_is_counted_rather_than_estimated() {
    // There is no price table in this crate, so an unpriced call leaves the
    // money alone and says how many it left alone.
    let (recorder, _) = recorder(Capture::default());
    recorder.model_call(1, ModelCall::new("acme", "acme-small", "solver"));

    let spend = recorder.accounting().run;
    assert_eq!(spend.cost(), None);
    assert_eq!(spend.unpriced_calls(), 1);
}

#[test]
fn every_model_call_carries_a_prompt_cache_hit_rate() {
    let (recorder, _) = recorder(Capture::default());
    let mut call = ModelCall::new("acme", "acme-small", "solver");
    call.prompt_tokens = 200;
    call.cached_tokens = 150;
    call.cache_hit_rate = ModelCall::hit_rate_from_tokens(200, 150);
    recorder.model_call(1, call);

    for entry in recorder.journal() {
        if let Event::ModelCalled { call, .. } = entry.event {
            assert!((call.cache_hit_rate - 0.75).abs() < f64::EPSILON);
        }
    }
    let rate = recorder.accounting().run.cache_hit_rate().unwrap();
    assert!((rate - 0.75).abs() < f64::EPSILON);
}

#[test]
fn a_hit_rate_for_an_empty_prompt_is_zero_rather_than_undefined() {
    assert!(ModelCall::hit_rate_from_tokens(0, 0).abs() < f64::EPSILON);
    // More cached than prompted is a provider bug; it must not exceed one.
    assert!((ModelCall::hit_rate_from_tokens(10, 40) - 1.0).abs() < f64::EPSILON);
}

#[test]
fn there_is_no_cache_hit_rate_before_the_first_call() {
    assert_eq!(Spend::default().cache_hit_rate(), None);
}

#[test]
fn two_concurrent_arms_report_a_concurrency_factor_above_one() {
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::PassStarted { pass: 1 });
    for arm in ["wide", "deep"] {
        recorder.record(Event::ArmStarted {
            pass: 1,
            arm: arm.to_string(),
        });
        recorder.record(Event::ArmFinished {
            pass: 1,
            arm: arm.to_string(),
            duration: Duration::from_millis(100),
        });
    }
    recorder.record(Event::PassFinished {
        pass: 1,
        duration: Duration::from_millis(120),
    });

    let report = recorder.report("goal", Outcome::Success, vec![Outcome::Success], vec![]);
    let profile = report.passes.iter().find(|p| p.pass == 1).unwrap();
    let factor = profile.concurrency_factor().unwrap();
    assert!(factor > 1.0, "expected parallelism, got {factor}");
    // A ratio, so nothing here can go negative the way "wall minus work" does.
    assert!(factor.is_sign_positive());
}

#[test]
fn a_pass_with_no_wall_clock_has_no_concurrency_factor() {
    assert_eq!(PassProfile::default().concurrency_factor(), None);
}

#[test]
fn the_summary_and_the_status_payload_come_from_one_report() {
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::PassStarted { pass: 1 });
    recorder.record(Event::Judged {
        pass: 1,
        judgement: Judgement::Proceed,
        score: 9,
    });
    recorder.record(Event::Routed {
        pass: 1,
        route: Route::Solved,
        reason: "verified twice".to_string(),
    });
    recorder.record(Event::StepFinished {
        pass: 1,
        step: "solve".to_string(),
        duration: Duration::from_millis(40),
    });
    recorder.record(Event::PassFinished {
        pass: 1,
        duration: Duration::from_millis(60),
    });
    recorder.record(Event::BoundTripped {
        pass: 1,
        bound: crate::Bound::ModelCalls,
    });

    let report = recorder.report(
        "land the change",
        Outcome::Success,
        vec![Outcome::Success, Outcome::Stalled],
        vec!["update the changelog".to_string()],
    );

    // The human summary and the payload a status call answers with are two
    // renderings of one value, which is what keeps them from diverging.
    let summary = report.summary();
    let payload = serde_json::to_value(&report).unwrap();
    assert_eq!(payload["goal"], "land the change");
    assert!(summary.contains("land the change"));
    assert!(summary.contains("solved"));
    assert!(summary.contains("model_calls"));
    assert!(summary.contains("update the changelog"));
    assert_eq!(report.routes, vec![Route::Solved]);
    assert_eq!(report.scores, vec![9]);
    assert_eq!(report.bound, Some(crate::Bound::ModelCalls));
    assert_eq!(report.steps.len(), 1);
    assert_eq!(
        serde_json::from_value::<Report>(payload).unwrap().summary(),
        summary,
    );
}

#[test]
fn a_report_states_repeat_reliability_rather_than_a_success_bit() {
    let (recorder, _) = recorder(Capture::default());
    let attempts = vec![
        Outcome::Success,
        Outcome::Stalled,
        Outcome::Success,
        Outcome::Blocked,
        Outcome::CleanNoOp,
        Outcome::Stalled,
        Outcome::Exhausted,
        Outcome::Success,
    ];
    let report = recorder.report("goal", Outcome::Success, attempts, vec![]);

    assert_eq!(report.attempts.len(), 8);
    assert_eq!(report.reliability(), Some(0.5));
    // Nothing in the payload is a lone success boolean.
    let payload = serde_json::to_value(&report).unwrap();
    assert!(payload.get("success").is_none());
    assert!(payload["attempts"].is_array());
}

#[test]
fn a_report_with_no_attempts_has_no_reliability() {
    let (recorder, _) = recorder(Capture::default());
    let report = recorder.report("goal", Outcome::Stalled, vec![], vec![]);

    assert_eq!(report.reliability(), None);
    assert!(report.summary().contains("n/a"));
}

#[test]
fn the_engines_node_activations_join_the_loops_own_stream_in_order() {
    let (recorder, _) = recorder(Capture::default());
    recorder.record(Event::PassStarted { pass: 2 });
    recorder.on_run_start("run-7");
    recorder.on_step_start("solve");
    let judge = recorder.child("judge");
    judge.model_call(2, ModelCall::new("acme", "acme-small", "judge"));
    recorder.on_step_finish(&ExecutionStep {
        node_id: "solve".to_string(),
        status: StepStatus::Error,
        duration_ms: 12,
        ..ExecutionStep::default()
    });
    recorder.on_run_finish(&Run {
        id: "run-7".to_string(),
        status: tinyflows::observability::RunStatus::CompletedWithErrors,
        steps: vec![ExecutionStep {
            node_id: "solve".to_string(),
            status: StepStatus::Error,
            ..ExecutionStep::default()
        }],
    });

    let journal = recorder.journal();
    let kinds: Vec<&str> = journal.iter().map(|entry| entry.event.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            "pass_started",
            "engine_run_started",
            "node_entered",
            "model_called",
            "node_finished",
            "engine_run_finished",
        ],
    );
    // Every plane's entry lands in the pass the loop announced, and carries the
    // label of the view that produced it.
    assert!(journal.iter().all(|entry| entry.event.pass() == 2));
    assert_eq!(journal[3].who, "judge");
    assert_eq!(journal[2].who, "loop");
    match &journal[4].event {
        Event::NodeFinished { duration, ok, .. } => {
            assert_eq!(*duration, Duration::from_millis(12));
            assert!(!ok, "an errored step must not report ok");
        }
        other => panic!("expected a node finish, got {other:?}"),
    }
    match &journal[5].event {
        Event::EngineRunFinished { failed, steps, .. } => {
            assert_eq!(failed, &vec!["solve".to_string()]);
            assert_eq!(*steps, 1);
        }
        other => panic!("expected an engine run finish, got {other:?}"),
    }
}

#[test]
fn a_fan_out_delivers_to_every_installed_sink() {
    let first = Arc::new(Collector::default());
    let second = Arc::new(Collector::default());
    let fan = FanOutSink::new()
        .with(first.clone())
        .with(second.clone());
    assert_eq!(fan.len(), 2);
    assert!(!fan.is_empty());

    fan.emit(&Event::PassStarted { pass: 1 });

    assert_eq!(first.events().len(), 1);
    assert_eq!(second.events().len(), 1);
}

#[test]
fn a_fan_out_with_no_sink_installed_drops_the_stream_without_failing() {
    // The run must not care whether anybody is listening.
    let fan = Arc::new(FanOutSink::new());
    assert!(fan.is_empty());
    let recorder = Recorder::new("loop", fan);

    recorder.record(Event::PassStarted { pass: 1 });

    // Nothing was emitted anywhere, and the journal still has the run.
    assert_eq!(recorder.journal().len(), 1);
}

#[test]
fn the_jsonl_sink_writes_one_line_per_event() {
    let buffer = Buffer::default();
    let sink = JsonlSink::new(buffer.clone());

    sink.emit(&Event::PassStarted { pass: 1 });
    sink.emit(&Event::LoopFinished {
        pass: 1,
        outcome: Outcome::Success,
    });

    let lines: Vec<&str> = buffer.contents().lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<Event>(lines[0]).unwrap(),
        Event::PassStarted { pass: 1 },
    );
    assert_eq!(sink.drops(), 0);
}

#[test]
fn a_sink_that_cannot_write_drops_the_entry_and_counts_it() {
    /// A writer that always fails, standing in for a full disk or a closed pipe.
    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let jsonl = JsonlSink::new(Broken);
    let line = LineSink::new(Broken);
    jsonl.emit(&Event::PassStarted { pass: 1 });
    line.emit(&Event::PassStarted { pass: 1 });

    assert_eq!(jsonl.drops(), 1);
    assert_eq!(line.drops(), 1);
}

#[test]
fn the_line_sink_renders_one_line_per_event() {
    let buffer = Buffer::default();
    let sink = LineSink::new(buffer.clone());

    sink.emit(&Event::StepEntered {
        pass: 2,
        step: "solve".to_string(),
    });
    sink.emit(&Event::StepFinished {
        pass: 2,
        step: "solve".to_string(),
        duration: Duration::from_millis(7),
    });

    let contents = buffer.contents();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines, vec!["pass 2 step solve entered", "pass 2 step solve finished in 7ms"]);
}

/// One of every event, so the rendering and the wire form are exercised whole
/// rather than variant by variant as somebody remembers to.
fn every_event() -> Vec<Event> {
    vec![
        Event::PassStarted { pass: 1 },
        Event::PassFinished {
            pass: 1,
            duration: Duration::from_millis(10),
        },
        Event::StepEntered {
            pass: 1,
            step: "solve".to_string(),
        },
        Event::StepFinished {
            pass: 1,
            step: "solve".to_string(),
            duration: Duration::from_millis(4),
        },
        Event::ArmStarted {
            pass: 1,
            arm: "wide".to_string(),
        },
        Event::ArmFinished {
            pass: 1,
            arm: "wide".to_string(),
            duration: Duration::from_millis(5),
        },
        Event::Merged {
            pass: 1,
            arms: 2,
            movement: Movement::from(&Delta {
                attempts: 1,
                established: 2,
                banked: 1,
                solved: Some(true),
                ..Delta::default()
            }),
        },
        Event::Judged {
            pass: 1,
            judgement: Judgement::Restart,
            score: 3,
        },
        Event::Routed {
            pass: 1,
            route: Route::Retry,
            reason: "nothing special happened".to_string(),
        },
        Event::Delegated {
            pass: 1,
            to: "reviewer".to_string(),
        },
        Event::DelegationFinished {
            pass: 1,
            to: "reviewer".to_string(),
            duration: Duration::from_secs(2),
        },
        Event::DirectiveReceived {
            pass: 1,
            directive: "stop after this pass".to_string(),
        },
        Event::BoundTripped {
            pass: 1,
            bound: crate::Bound::RunClock,
        },
        Event::LoopFinished {
            pass: 1,
            outcome: Outcome::Exhausted,
        },
        Event::ModelCalled {
            pass: 1,
            call: ModelCall::new("acme", "acme-small", "solver"),
        },
        Event::ToolCalled {
            pass: 1,
            call: ToolCall::new("shell", "solver"),
        },
        Event::NodeEntered {
            pass: 1,
            node: "solve".to_string(),
        },
        Event::NodeFinished {
            pass: 1,
            node: "solve".to_string(),
            duration: Duration::from_millis(1),
            ok: true,
        },
        Event::EngineRunStarted {
            pass: 1,
            run: "run-1".to_string(),
        },
        Event::EngineRunFinished {
            pass: 1,
            run: "run-1".to_string(),
            steps: 3,
            failed: vec![],
        },
    ]
}

#[test]
fn every_event_names_its_pass_and_renders_to_one_line() {
    for event in every_event() {
        assert_eq!(event.pass(), 1);
        let line = render(&event);
        assert!(line.starts_with("pass 1"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        assert!(!event.kind().is_empty());
    }
}

#[test]
fn every_event_round_trips_through_its_wire_form() {
    // The event names are a wire format: a journal outlives the process that
    // wrote it, so a renamed variant must fail here rather than out there.
    for event in every_event() {
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], event.kind());
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }
}

#[test]
fn a_merge_carries_the_movement_it_applied() {
    let delta = Delta {
        attempts: 1,
        unproductive: -1,
        banked: 2,
        expired: Some(false),
        ..Delta::default()
    };
    let movement = Movement::from(&delta);

    assert_eq!(movement.attempts, 1);
    assert_eq!(movement.unproductive, -1);
    assert_eq!(movement.banked, 2);
    assert_eq!(movement.expired, Some(false));
}

#[test]
fn only_the_spine_events_are_spine_events() {
    for event in every_event() {
        let expected = matches!(
            event.kind(),
            "pass_started"
                | "pass_finished"
                | "judged"
                | "routed"
                | "bound_tripped"
                | "loop_finished"
        );
        assert_eq!(event.is_spine(), expected, "{}", event.kind());
    }
}
