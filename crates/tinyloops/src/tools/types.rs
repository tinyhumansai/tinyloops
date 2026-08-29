//! The call boundary a tool is reached across, and the vocabulary a failure is
//! sorted into.
//!
//! Everything here is data plus the small amount of arithmetic that keeps it
//! honest. The registry that decides which of these tools exist at all, and the
//! decorator that keeps a failure model-readable, live in the module root: both
//! are rules *about* tools rather than properties of any one of them.
//!
//! The shapes deliberately mirror the durable harness's own tool layer
//! (`vendor/tinyagents/src/harness/tool/types.rs`), because the two have to
//! agree about what a call is. Most of all [`ToolInvocation::invalid`]: a model
//! that emits argument JSON nobody can parse has not ended the call, and the
//! raw string it emitted is preserved so it can be handed back as an error
//! result and retried.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The four verbs a tool set is reviewed on.
///
/// Every surveyed agent converges on read, search, edit and execute, whether it
/// exposes one tool or thirty-seven. A [`ToolSet`](super::ToolSet) is therefore
/// judged on whether these are cleanly separable, not on how many entries it
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGroup {
    /// Reading a named thing back.
    Read,
    /// Finding the name in the first place.
    Search,
    /// Changing what a name holds.
    Edit,
    /// Running something and reporting what it printed.
    Execute,
}

impl ToolGroup {
    /// Every group, in a fixed order.
    pub const ALL: [Self; 4] = [Self::Read, Self::Search, Self::Edit, Self::Execute];

    /// The group's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Search => "search",
            Self::Edit => "edit",
            Self::Execute => "execute",
        }
    }
}

/// Which groups an attempt was granted.
///
/// A grant is read once, in [`ToolSet::new`](super::ToolSet::new). It is not
/// carried into a handler, because a handler that can ask whether it is allowed
/// to run is a handler some call path can reach anyway.
///
/// A set rather than four flags: a grant is the list of groups that exist for
/// this attempt, and reading it as a list is what keeps "withheld" and "absent"
/// the same word.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolGrant {
    groups: BTreeSet<ToolGroup>,
}

impl ToolGrant {
    /// A grant of exactly the named groups.
    #[must_use]
    pub fn of(groups: &[ToolGroup]) -> Self {
        Self {
            groups: groups.iter().copied().collect(),
        }
    }

    /// A grant of every group.
    #[must_use]
    pub fn all() -> Self {
        Self::of(&ToolGroup::ALL)
    }

    /// A grant that may look but not touch.
    #[must_use]
    pub fn read_only() -> Self {
        Self::of(&[ToolGroup::Read, ToolGroup::Search])
    }

    /// Whether `group` is granted.
    #[must_use]
    pub fn holds(&self, group: ToolGroup) -> bool {
        self.groups.contains(&group)
    }

    /// The granted groups, in order.
    #[must_use]
    pub fn groups(&self) -> Vec<ToolGroup> {
        self.groups.iter().copied().collect()
    }
}

/// A model-visible declaration of one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// The tool's canonical name.
    pub name: String,
    /// What the tool does, in the words the model reads.
    pub description: String,
    /// A JSON Schema object describing the arguments.
    pub parameters: Value,
}

impl ToolSchema {
    /// Builds a schema from a name, a description, and named string arguments.
    ///
    /// Every argument is required; the reference tools have no optional ones,
    /// and a schema builder that guesses optionality is a schema builder that
    /// is wrong somewhere nobody looks.
    #[must_use]
    pub fn new(name: &str, description: &str, arguments: &[&str]) -> Self {
        let mut properties = Map::new();
        for argument in arguments {
            let mut field = Map::new();
            field.insert("type".to_owned(), Value::String("string".to_owned()));
            properties.insert((*argument).to_owned(), Value::Object(field));
        }
        let required = arguments
            .iter()
            .map(|argument| Value::String((*argument).to_owned()))
            .collect::<Vec<_>>();
        let mut parameters = Map::new();
        parameters.insert("type".to_owned(), Value::String("object".to_owned()));
        parameters.insert("properties".to_owned(), Value::Object(properties));
        parameters.insert("required".to_owned(), Value::Array(required));
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: Value::Object(parameters),
        }
    }

    /// Returns this schema with `injected` projected out of it.
    ///
    /// Removed from `properties` and from `required` alike, so a host-supplied
    /// argument is neither advertised to the model nor demanded of it. This is
    /// the difference between [`ToolSet::schemas`](super::ToolSet::schemas) and
    /// [`ToolSet::declared_schemas`](super::ToolSet::declared_schemas).
    #[must_use]
    pub fn without(mut self, injected: &[&'static str]) -> Self {
        if let Some(object) = self.parameters.as_object_mut() {
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                for name in injected {
                    properties.remove(*name);
                }
            }
            if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                required
                    .retain(|value| value.as_str().is_none_or(|name| !injected.contains(&name)));
            }
        }
        self
    }
}

/// A request to invoke one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// The call id a result is correlated back to.
    pub id: String,
    /// The tool the call names.
    pub name: String,
    /// The arguments, as JSON.
    ///
    /// When [`Self::invalid`] is set this holds the raw, unparseable string the
    /// model emitted, as a JSON string, rather than a parsed object.
    pub arguments: Value,
    /// Set when the emitted arguments could not be parsed as JSON.
    ///
    /// A small local model occasionally emits malformed argument JSON. That is
    /// not a reason to end the call: the raw string is preserved above, this
    /// field carries the parse error, and the registry answers with a
    /// model-readable error result the model can retry against.
    pub invalid: Option<String>,
}

impl ToolInvocation {
    /// A call whose arguments already parsed.
    #[must_use]
    pub fn new(id: &str, name: &str, arguments: Value) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            invalid: None,
        }
    }

    /// A call built from the raw argument string a model emitted.
    ///
    /// On a parse failure the raw string is kept and [`Self::invalid`] carries
    /// the reason, so the failure reaches the model as a message rather than
    /// ending the run.
    #[must_use]
    pub fn parsed(id: &str, name: &str, raw: &str) -> Self {
        match serde_json::from_str::<Value>(raw) {
            Ok(arguments) => Self::new(id, name, arguments),
            Err(error) => Self {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments: Value::String(raw.to_owned()),
                invalid: Some(error.to_string()),
            },
        }
    }

    /// Reads a string argument, or the empty string when it is absent.
    #[must_use]
    pub fn argument(&self, name: &str) -> &str {
        self.arguments
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// What a failed tool call may still be turned into.
///
/// The sort is the whole point: only one of these three ends anything, and the
/// other two keep a run that would otherwise have stopped moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Feed the error back as a message, against a bounded retry count.
    Requery,
    /// Reconstruct what can be reconstructed.
    ///
    /// The canonical case is rebuilding a diff from the trajectory when the
    /// sandbox is dead, so a dead environment still yields a result instead of
    /// nothing.
    Salvage,
    /// End the step. The only variant that ends anything.
    Fatal,
}

impl Recovery {
    /// The recovery's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requery => "requery",
            Self::Salvage => "salvage",
            Self::Fatal => "fatal",
        }
    }
}

/// A tool's own failure, before anything has decided what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// What went wrong, in the words the model will read.
    pub message: String,
    /// What the tool believes can still be made of the failure.
    pub recovery: Recovery,
    /// What was reconstructed, when the recovery is [`Recovery::Salvage`].
    pub salvage: Option<String>,
}

impl ToolError {
    /// A failure worth asking the model about again.
    #[must_use]
    pub fn requery(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recovery: Recovery::Requery,
            salvage: None,
        }
    }

    /// A failure that still produced something usable.
    #[must_use]
    pub fn salvaged(message: impl Into<String>, salvage: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recovery: Recovery::Salvage,
            salvage: Some(salvage.into()),
        }
    }

    /// A failure nothing can be made of.
    #[must_use]
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recovery: Recovery::Fatal,
            salvage: None,
        }
    }

    /// The text a model should see for this failure.
    ///
    /// A salvage reports what it rebuilt, because the point of salvaging is
    /// that the caller receives a result; the others report the error itself.
    #[must_use]
    pub fn model_readable(&self) -> String {
        match &self.salvage {
            Some(salvage) => format!("{} (salvaged) {salvage}", self.message),
            None => format!("tool error: {}", self.message),
        }
    }
}

/// What a tool returns when it returns at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReport {
    /// The model-facing content.
    pub content: String,
    /// The recovery a decorator applied, when the call actually failed.
    ///
    /// `None` on a plain success. `Some` means the content above was
    /// manufactured from a failure by [`Resilient`](super::Resilient), which is
    /// how both the harness path and the capability path observe the same
    /// decoration.
    pub recovery: Option<Recovery>,
}

impl ToolReport {
    /// A plain success.
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recovery: None,
        }
    }

    /// A failure a decorator turned into a readable result.
    #[must_use]
    pub fn recovered(content: impl Into<String>, recovery: Recovery) -> Self {
        Self {
            content: content.into(),
            recovery: Some(recovery),
        }
    }
}

/// Proof that a named tool executed and produced the output being recorded.
///
/// The ledger's `evidence_origin` distinction rests on this type: only the
/// executing tool may call something `collected`, and this receipt is the only
/// thing that says a tool executed. It is minted inside
/// [`ToolSet::invoke`](super::ToolSet::invoke) and nowhere else — there is no
/// public constructor — so a transcript somebody handed the run cannot acquire
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReceipt {
    tool: String,
}

impl ToolReceipt {
    /// Mints a receipt for the tool that just ran.
    ///
    /// Visible to `tools` alone rather than to the crate, so
    /// `Evidence::collected` cannot be satisfied by a module that did not run
    /// anything. A transcript you were given is a claim; a transcript you
    /// collected is evidence, and `pub(crate)` would have left every other
    /// module able to mint the difference away.
    pub(super) fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_owned(),
        }
    }

    /// The tool that ran.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }
}

/// The outcome of one call through the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The call this answers.
    pub call_id: String,
    /// The tool that produced it.
    pub name: String,
    /// The model-facing content, whether the call succeeded or was recovered.
    pub content: String,
    /// The recovery applied, when the call failed and was made readable.
    pub recovery: Option<Recovery>,
    receipt: ToolReceipt,
}

impl ToolOutcome {
    /// Builds an outcome around the receipt for the tool that ran.
    pub(crate) fn new(
        call_id: &str,
        name: &str,
        content: String,
        recovery: Option<Recovery>,
    ) -> Self {
        Self {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            content,
            recovery,
            receipt: ToolReceipt::new(name),
        }
    }

    /// Whether this outcome was manufactured from a failure.
    #[must_use]
    pub const fn is_recovered(&self) -> bool {
        self.recovery.is_some()
    }

    /// The receipt proving the tool executed.
    #[must_use]
    pub const fn receipt(&self) -> &ToolReceipt {
        &self.receipt
    }
}

/// One line of the history a failure travels in.
///
/// Failures are messages, never out-of-band state: the next model call sees
/// what failed, and the recorded history explains the retry to a person reading
/// it afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMessage {
    /// The call the message belongs to.
    pub call_id: String,
    /// The tool that failed.
    pub tool: String,
    /// The model-facing text.
    pub text: String,
    /// How the failure was sorted.
    pub recovery: Recovery,
}
