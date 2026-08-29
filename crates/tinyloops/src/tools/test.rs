//! Unit tests for the tool seam.
//!
//! Three properties are pinned here, and each one is pinned because its failure
//! is silent:
//!
//! - **a withheld tool is absent**, not discouraged. No test in this file
//!   achieves the absence of a tool by prompt text, and a later contributor
//!   should not add one: an instruction is advice to a sampler, an unregistered
//!   tool is a call that cannot be made;
//! - **the decorator is on the instance**, so the harness path and the
//!   capability path observe the same behavior — a decorator applied at
//!   registration is simply absent on the second path and nothing says so;
//! - **the schema sets stay apart**, because a flattened list advertises a
//!   host-supplied argument to a model, which then supplies it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use super::*;
use crate::Error;

/// A tool that always fails the way the test asks it to.
#[derive(Debug)]
struct FailingTool {
    error: ToolError,
}

impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        "execute"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new("execute", "always fails", &["command"])
    }

    fn invoke(&self, _call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        Err(self.error.clone())
    }
}

/// A set whose execute group is the failing tool above.
fn set_with_failing_execute(error: ToolError) -> ToolSet {
    let mut groups = PureTools::groups();
    groups.execute = Arc::new(FailingTool { error });
    ToolSet::from_groups(ToolGrant::all(), groups)
}

/// The call the reference execute tool answers.
fn execute_call(command: &str) -> ToolInvocation {
    ToolInvocation::new("call-1", "execute", json!({ "command": command }))
}

#[test]
fn constructing_without_a_group_leaves_it_absent_from_schemas() {
    let set = ToolSet::new(ToolGrant::read_only());

    let names = set.names();
    assert!(names.contains(&"read".to_owned()));
    assert!(!names.contains(&"execute".to_owned()));
    assert!(
        set.schemas().iter().all(|schema| schema.name != "execute"),
        "a withheld group is not in the model-facing schemas"
    );
    assert!(set.tool("execute").is_none());
}

#[test]
fn an_unregistered_tool_is_an_error_rather_than_a_refusal_the_model_could_argue_with() {
    let set = ToolSet::new(ToolGrant::read_only());

    assert_eq!(
        set.invoke(&execute_call("ls")).unwrap_err(),
        Error::UnknownTool {
            name: "execute".to_owned()
        }
    );
}

#[test]
fn no_handler_decides_whether_it_is_allowed_to_run() {
    // The reference execute tool, taken undecorated and invoked directly with
    // no grant anywhere in sight, runs. Enforcement therefore cannot live in a
    // handler; it lives in `ToolSet::new`, which is the point.
    let execute = PureTools::groups().execute;

    assert_eq!(
        execute.invoke(&execute_call("cargo test")).unwrap(),
        ToolReport::ok("ran cargo test")
    );
}

#[test]
fn the_model_facing_and_introspection_schema_sets_differ() {
    let set = ToolSet::new(ToolGrant::all());

    let model_facing = set
        .schemas()
        .into_iter()
        .find(|schema| schema.name == "execute")
        .unwrap();
    let declared = set
        .declared_schemas()
        .into_iter()
        .find(|schema| schema.name == "execute")
        .unwrap();

    assert_ne!(model_facing, declared);
    assert!(declared.parameters["properties"].get("sandbox").is_some());
    assert!(
        model_facing.parameters["properties"]
            .get("sandbox")
            .is_none()
    );
    let required = model_facing.parameters["required"].as_array().unwrap();
    assert!(!required.contains(&Value::String("sandbox".to_owned())));
}

#[test]
fn declared_schemas_output_never_reaches_a_model_request() {
    let set = ToolSet::new(ToolGrant::all());

    // The request a provider adapter would send is built from `schemas()`.
    let request = json!({ "tools": set.schemas() }).to_string();

    assert!(
        !request.contains("sandbox"),
        "an injected argument reached the wire: {request}"
    );
    assert!(
        json!({ "tools": set.declared_schemas() })
            .to_string()
            .contains("sandbox"),
        "the introspection view is the one that keeps it"
    );
}

#[test]
fn a_tool_error_becomes_a_model_readable_result() {
    let set = ToolSet::new(ToolGrant::all());

    let outcome = set
        .invoke(&ToolInvocation::new(
            "call-1",
            "read",
            json!({ "path": "missing.md" }),
        ))
        .unwrap();

    assert_eq!(outcome.recovery, Some(Recovery::Requery));
    assert_eq!(outcome.content, "tool error: no document named missing.md");
    assert!(outcome.is_recovered());
}

#[test]
fn the_same_instance_behaves_identically_through_both_paths() {
    let set = ToolSet::new(ToolGrant::all());
    let call = ToolInvocation::new("call-1", "read", json!({ "path": "missing.md" }));

    // The capability path: a `tool_call` node takes the instance and invokes it
    // directly, with no middleware stack in between.
    let capability = set.tool("read").unwrap().invoke(&call).unwrap();
    // The harness path: the registry invokes the same instance.
    let harness = set.invoke(&call).unwrap();

    assert_eq!(capability.recovery, Some(Recovery::Requery));
    assert_eq!(capability.content, harness.content);
    assert_eq!(capability.recovery, harness.recovery);
}

#[test]
fn requery_feeds_the_error_back_against_a_bounded_retry_count() {
    let set = set_with_failing_execute(ToolError::requery("no")).with_max_requeries(2);

    assert!(set.invoke(&execute_call("one")).is_ok());
    assert!(set.invoke(&execute_call("two")).is_ok());
    assert_eq!(
        set.invoke(&execute_call("three")).unwrap_err(),
        Error::RequeriesExhausted {
            tool: "execute".to_owned(),
            limit: 2,
        }
    );
}

#[test]
fn a_dead_sandbox_fixture_salvages_a_reconstructed_diff() {
    let set = ToolSet::new(ToolGrant::all());

    let outcome = set
        .invoke(&ToolInvocation::new(
            "call-1",
            "execute",
            json!({ "command": "cargo test", "sandbox": "dead", "trajectory": "edited plan.md" }),
        ))
        .unwrap();

    assert_eq!(outcome.recovery, Some(Recovery::Salvage));
    assert!(
        outcome.content.contains("edited plan.md"),
        "a dead environment still yields a result: {}",
        outcome.content
    );
}

#[test]
fn fatal_is_the_only_variant_that_ends_a_step() {
    let set = ToolSet::new(ToolGrant::all());

    let fatal = set.invoke(&ToolInvocation::new(
        "call-1",
        "execute",
        json!({ "command": "cargo test", "sandbox": "dead" }),
    ));

    assert_eq!(
        fatal.unwrap_err(),
        Error::ToolFatal {
            tool: "execute".to_owned(),
            message: "the sandbox is gone and left nothing".to_owned(),
        }
    );
    // The recoverable variants did not end anything.
    assert!(set.invoke(&execute_call("cargo build")).is_ok());
}

#[test]
fn every_failure_appears_in_the_history_as_a_message() {
    let set = ToolSet::new(ToolGrant::all());

    let _ = set.invoke(&ToolInvocation::new(
        "call-1",
        "read",
        json!({ "path": "missing.md" }),
    ));
    let _ = set.invoke(&ToolInvocation::new(
        "call-2",
        "execute",
        json!({ "command": "x", "sandbox": "dead" }),
    ));

    let history = set.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].call_id, "call-1");
    assert_eq!(history[0].tool, "read");
    assert_eq!(history[0].recovery, Recovery::Requery);
    assert_eq!(history[1].recovery, Recovery::Fatal);
    assert!(history[1].text.contains("the sandbox is gone"));
}

#[test]
fn arguments_that_do_not_parse_are_fed_back_rather_than_failing_the_call() {
    let set = ToolSet::new(ToolGrant::all());
    let call = ToolInvocation::parsed("call-1", "read", "{path: notes.md");

    assert!(call.invalid.is_some());
    assert_eq!(call.arguments, Value::String("{path: notes.md".to_owned()));

    let outcome = set.invoke(&call).unwrap();
    assert_eq!(outcome.recovery, Some(Recovery::Requery));
    assert!(outcome.content.contains("arguments did not parse"));
    assert_eq!(set.history().len(), 1);
}

#[test]
fn well_formed_arguments_parse_into_a_valid_call() {
    let call = ToolInvocation::parsed("call-1", "read", r#"{"path":"notes.md"}"#);

    assert_eq!(call.invalid, None);
    assert_eq!(call.argument("path"), "notes.md");
    assert_eq!(call.argument("absent"), "");
}

#[test]
fn the_reference_set_separates_read_search_edit_and_execute() {
    let set = ToolSet::new(ToolGrant::all());

    assert_eq!(set.names(), vec!["read", "search", "edit", "execute"]);
    assert_eq!(
        set.invoke(&ToolInvocation::new(
            "c",
            "read",
            json!({ "path": "plan.md" })
        ))
        .unwrap()
        .content,
        "attempt, evaluate, route, budget"
    );
    assert_eq!(
        set.invoke(&ToolInvocation::new(
            "c",
            "search",
            json!({ "term": "loop" })
        ))
        .unwrap()
        .content,
        "notes.md, readme.md"
    );
    assert_eq!(
        set.invoke(&ToolInvocation::new(
            "c",
            "edit",
            json!({ "path": "plan.md", "from": "budget", "to": "stop" })
        ))
        .unwrap()
        .content,
        "attempt, evaluate, route, stop"
    );
    assert_eq!(
        set.invoke(&execute_call("cargo test")).unwrap().content,
        "ran cargo test"
    );
}

#[test]
fn the_same_arguments_produce_the_same_result() {
    let first = ToolSet::new(ToolGrant::all());
    let second = ToolSet::new(ToolGrant::all());
    let call = ToolInvocation::new("c", "search", json!({ "term": "loop" }));

    assert_eq!(
        first.invoke(&call).unwrap().content,
        second.invoke(&call).unwrap().content
    );
}

#[test]
fn each_reference_tool_reports_its_own_bad_arguments() {
    let set = ToolSet::new(ToolGrant::all());

    let empty_term = set
        .invoke(&ToolInvocation::new("c", "search", json!({ "term": "" })))
        .unwrap();
    assert_eq!(empty_term.content, "tool error: search needs a term");

    let unknown_document = set
        .invoke(&ToolInvocation::new(
            "c",
            "edit",
            json!({ "path": "nope.md", "from": "a", "to": "b" }),
        ))
        .unwrap();
    assert_eq!(
        unknown_document.content,
        "tool error: no document named nope.md"
    );

    let absent_fragment = set
        .invoke(&ToolInvocation::new(
            "c",
            "edit",
            json!({ "path": "plan.md", "from": "zebra", "to": "b" }),
        ))
        .unwrap();
    assert_eq!(
        absent_fragment.content,
        "tool error: plan.md does not hold zebra"
    );

    let no_command = set
        .invoke(&ToolInvocation::new(
            "c",
            "execute",
            json!({ "command": "" }),
        ))
        .unwrap();
    assert_eq!(no_command.content, "tool error: execute needs a command");
}

#[test]
fn a_receipt_names_the_tool_that_ran() {
    let set = ToolSet::new(ToolGrant::all());

    let outcome = set
        .invoke(&ToolInvocation::new(
            "c",
            "read",
            json!({ "path": "notes.md" }),
        ))
        .unwrap();

    assert_eq!(outcome.receipt().tool(), "read");
    assert!(!outcome.is_recovered());
    assert_eq!(outcome.call_id, "c");
    assert_eq!(outcome.name, "read");
}

#[test]
fn a_grant_holds_exactly_the_groups_it_names() {
    let grant = ToolGrant::read_only();

    assert!(grant.holds(ToolGroup::Read));
    assert!(grant.holds(ToolGroup::Search));
    assert!(!grant.holds(ToolGroup::Edit));
    assert!(!grant.holds(ToolGroup::Execute));
    assert_eq!(ToolSet::new(grant.clone()).grant(), &grant);
    assert_eq!(grant.groups(), vec![ToolGroup::Read, ToolGroup::Search]);
    assert_eq!(ToolGrant::default().groups(), Vec::new());
    assert!(!ToolGrant::default().holds(ToolGroup::Read));
}

#[test]
fn the_names_of_the_vocabulary_are_the_wire_names() {
    assert_eq!(
        ToolGroup::ALL.map(ToolGroup::as_str),
        ["read", "search", "edit", "execute"]
    );
    assert_eq!(Recovery::Requery.as_str(), "requery");
    assert_eq!(Recovery::Salvage.as_str(), "salvage");
    assert_eq!(Recovery::Fatal.as_str(), "fatal");
    assert_eq!(
        serde_json::to_string(&ToolGroup::Execute).unwrap(),
        "\"execute\""
    );
    assert_eq!(
        serde_json::from_str::<Recovery>("\"salvage\"").unwrap(),
        Recovery::Salvage
    );
    assert_eq!(
        serde_json::to_string(&ToolGrant::read_only()).unwrap(),
        r#"["read","search"]"#
    );
    assert!(
        serde_json::from_str::<ToolGrant>(r#"["execute"]"#)
            .unwrap()
            .holds(ToolGroup::Execute)
    );
    assert_eq!(
        serde_json::from_str::<ToolGroup>("\"read\"").unwrap(),
        ToolGroup::Read
    );
}

#[test]
fn a_call_round_trips_through_its_wire_form() {
    let call = ToolInvocation::new("c", "read", json!({ "path": "notes.md" }));
    let encoded = serde_json::to_string(&call).unwrap();

    assert_eq!(
        serde_json::from_str::<ToolInvocation>(&encoded).unwrap(),
        call
    );

    let schema = ToolSchema::new("read", "read one document", &["path"]);
    let encoded = serde_json::to_string(&schema).unwrap();
    assert_eq!(
        serde_json::from_str::<ToolSchema>(&encoded).unwrap(),
        schema
    );
}

#[test]
fn a_salvage_reports_what_it_rebuilt_and_a_requery_reports_the_error() {
    let salvaged = ToolError::salvaged("the sandbox is gone", "a diff");
    let requery = ToolError::requery("no such thing");
    let fatal = ToolError::fatal("nothing left");

    assert_eq!(
        salvaged.model_readable(),
        "the sandbox is gone (salvaged) a diff"
    );
    assert_eq!(requery.model_readable(), "tool error: no such thing");
    assert_eq!(fatal.recovery, Recovery::Fatal);
    assert_eq!(fatal.salvage, None);
    assert_eq!(
        ToolReport::recovered("x", Recovery::Salvage).recovery,
        Some(Recovery::Salvage)
    );
    assert_eq!(ToolReport::ok("x").recovery, None);
}

#[test]
fn projecting_an_injected_argument_out_of_a_schema_with_no_object_is_a_no_op() {
    let mut schema = ToolSchema::new("read", "read", &["path"]);
    schema.parameters = Value::Null;

    assert_eq!(schema.clone().without(&["path"]).parameters, Value::Null);
}

#[test]
fn the_default_requery_bound_is_the_documented_one() {
    let set = set_with_failing_execute(ToolError::requery("no"));

    for _ in 0..MAX_REQUERIES {
        assert!(set.invoke(&execute_call("again")).is_ok());
    }
    assert!(matches!(
        set.invoke(&execute_call("again")).unwrap_err(),
        Error::RequeriesExhausted { limit, .. } if limit == MAX_REQUERIES
    ));
}

#[test]
fn a_tool_renders_its_name_in_debug_output() {
    let tool = PureTools::groups().read;

    assert!(format!("{tool:?}").contains("read"));
}

#[test]
fn the_tool_failures_render_the_messages_a_reader_will_see() {
    assert_eq!(
        Error::UnknownTool {
            name: "execute".to_owned()
        }
        .to_string(),
        "no tool named execute is registered"
    );
    assert_eq!(
        Error::ToolFatal {
            tool: "execute".to_owned(),
            message: "the sandbox is gone".to_owned(),
        }
        .to_string(),
        "tool execute failed fatally: the sandbox is gone"
    );
    assert_eq!(
        Error::RequeriesExhausted {
            tool: "execute".to_owned(),
            limit: 3,
        }
        .to_string(),
        "tool execute exhausted its 3 requeries"
    );
}
