//! The tool seam: the registry an attempt is handed, and what happens when a
//! tool fails.
//!
//! # A withheld tool is withheld by not registering it
//!
//! That is the governing rule of this module, and it is why [`ToolSet::new`]
//! takes a [`ToolGrant`] and returns a struct of optional groups. A tool the
//! attempt may not use is *absent*: it is not in [`ToolSet::schemas`], the
//! model never learns it exists, and [`ToolSet::invoke`] cannot reach it.
//!
//! A prompt instruction is not a control. No test in this module achieves the
//! absence of a tool by asking a model to abstain, and a later contributor
//! should not add one — an instruction is advice to a sampler, while an
//! unregistered tool is a call that cannot be made. For the same reason no
//! handler here takes a grant: a handler that can ask whether it is allowed to
//! run is a handler some call path reaches anyway.
//!
//! # The decorator is applied at construction, not at registration
//!
//! [`Resilient`] wraps a tool *instance* before that instance is shared. The
//! same instances are handed to two callers: the harness, which runs a
//! middleware stack, and the workflow capability path, where a `tool_call` node
//! reaches `Capabilities::tools` directly and there is no stack to run anything
//! through. A decorator applied when a tool is registered with the harness is
//! simply absent on the second path, and the two callers then disagree about
//! what the same tool does. Wrapping the instance is what makes the two agree,
//! and a test asserts it on both paths.
//!
//! # Failure sorts into a typed [`Recovery`]
//!
//! [`Recovery::Requery`] feeds the error back as a message against a bounded
//! retry count — bounded, because an unbounded requery is a loop that spends
//! its budget re-asking the same broken question. [`Recovery::Salvage`]
//! reconstructs what it can, the canonical case being a diff rebuilt from the
//! trajectory once the sandbox is dead, so a dead environment still yields a
//! result. [`Recovery::Fatal`] is the only variant that ends anything.
//!
//! Errors travel as messages in [`ToolSet::history`], never as out-of-band
//! state, so the next model call can see what failed and the recorded history
//! explains the retry.
//!
//! # Two schema sets, kept apart
//!
//! [`ToolSet::schemas`] projects injected arguments out of what the model sees.
//! [`ToolSet::declared_schemas`] is the introspection view — registry listings,
//! audit logs, docs — and never goes on the wire. Flattening the two into one
//! list advertises a host-supplied argument to a model, which then supplies it,
//! which is a call the host did not mean to be able to make.

mod types;

pub use types::{
    Recovery, ToolError, ToolGrant, ToolGroup, ToolInvocation, ToolMessage, ToolOutcome,
    ToolReceipt, ToolReport, ToolSchema,
};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{Error, Result};

/// How many times one tool's failure may be fed back before it becomes fatal.
///
/// The bound is the point. A requery that never runs out turns one broken tool
/// into a run that spends its whole budget asking the same question, and the
/// run reports nothing because it never got anywhere else.
pub const MAX_REQUERIES: u32 = 3;

/// One tool: a name, a declaration, and something it does.
///
/// Synchronous, matching `step::Step`, for the same reason given there: the
/// adapter onto the engine's asynchronous `ToolInvoker` belongs with the
/// dependency that introduces it, and the decision a tool makes is not made any
/// more correct by being awaited.
pub trait Tool: Send + Sync {
    /// The tool's canonical name.
    fn name(&self) -> &str;

    /// The tool's declared schema, injected arguments included.
    fn schema(&self) -> ToolSchema;

    /// The argument names the host supplies rather than the model.
    ///
    /// Projected out of [`ToolSet::schemas`] and kept in
    /// [`ToolSet::declared_schemas`].
    fn injected_arguments(&self) -> &[&'static str] {
        &[]
    }

    /// Runs the tool.
    ///
    /// The signature takes no grant on purpose: whether this tool may run was
    /// decided in [`ToolSet::new`], before the instance existed.
    ///
    /// # Errors
    ///
    /// Returns a [`ToolError`] carrying the recovery the tool believes applies.
    /// Wrapped in [`Resilient`], only [`Recovery::Fatal`] escapes as an error.
    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError>;
}

/// The decorator that turns a tool error into a model-readable result.
///
/// Applied when the instance is built, before it is shared with either caller —
/// see the module docs for why registration is too late.
#[derive(Debug)]
pub struct Resilient {
    inner: Arc<dyn Tool>,
}

impl Resilient {
    /// Wraps `inner`, returning the instance both callers will share.
    #[must_use]
    pub fn wrap(inner: Arc<dyn Tool>) -> Arc<dyn Tool> {
        Arc::new(Self { inner })
    }
}

impl std::fmt::Debug for dyn Tool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Tool")
            .field("name", &self.name())
            .finish()
    }
}

impl Tool for Resilient {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn schema(&self) -> ToolSchema {
        self.inner.schema()
    }

    fn injected_arguments(&self) -> &[&'static str] {
        self.inner.injected_arguments()
    }

    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        match self.inner.invoke(call) {
            Ok(report) => Ok(report),
            Err(error) if error.recovery == Recovery::Fatal => Err(error),
            Err(error) => Ok(ToolReport::recovered(
                error.model_readable(),
                error.recovery,
            )),
        }
    }
}

/// The registry facade an attempt is handed.
///
/// Constructed once per attempt from a grant. The groups it does not hold are
/// `None`, and nothing in the public surface can add one afterwards.
#[derive(Debug)]
pub struct ToolSet {
    grant: ToolGrant,
    groups: BTreeMap<ToolGroup, Arc<dyn Tool>>,
    requeries: Mutex<BTreeMap<String, u32>>,
    history: Mutex<Vec<ToolMessage>>,
    max_requeries: u32,
}

/// The four instances a [`ToolSet`] is assembled from.
///
/// A deployment supplies its own; [`PureTools::groups`] supplies the offline
/// reference set every bundled example runs on.
#[derive(Debug, Clone)]
pub struct ToolGroups {
    /// The tool that reads a named thing back.
    pub read: Arc<dyn Tool>,
    /// The tool that finds a name.
    pub search: Arc<dyn Tool>,
    /// The tool that changes what a name holds.
    pub edit: Arc<dyn Tool>,
    /// The tool that runs something.
    pub execute: Arc<dyn Tool>,
}

/// Locks `mutex`, taking the value back from a poisoned lock.
///
/// A panic in one tool must not stop the run's tool history from being written:
/// a partially appended history is still the run's history, and losing it is
/// strictly worse than continuing with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ToolSet {
    /// Builds a set of the granted groups over the offline reference tools.
    #[must_use]
    pub fn new(grant: ToolGrant) -> Self {
        Self::from_groups(grant, PureTools::groups())
    }

    /// Builds a set of the granted groups over supplied instances.
    ///
    /// Every instance is wrapped in [`Resilient`] here, at construction, so the
    /// instance this set hands to the capability path and the instance it calls
    /// itself are the same decorated object.
    #[must_use]
    pub fn from_groups(grant: ToolGrant, groups: ToolGroups) -> Self {
        let mut registered: BTreeMap<ToolGroup, Arc<dyn Tool>> = BTreeMap::new();
        let candidates = [
            (ToolGroup::Read, groups.read),
            (ToolGroup::Search, groups.search),
            (ToolGroup::Edit, groups.edit),
            (ToolGroup::Execute, groups.execute),
        ];
        for (group, tool) in candidates {
            if grant.holds(group) {
                registered.insert(group, Resilient::wrap(tool));
            }
        }
        Self {
            grant,
            groups: registered,
            requeries: Mutex::new(BTreeMap::new()),
            history: Mutex::new(Vec::new()),
            max_requeries: MAX_REQUERIES,
        }
    }

    /// The grant this set was constructed from.
    #[must_use]
    pub const fn grant(&self) -> ToolGrant {
        self.grant
    }

    /// Replaces the requery bound, for a deployment that wants a tighter one.
    #[must_use]
    pub fn with_max_requeries(mut self, max_requeries: u32) -> Self {
        self.max_requeries = max_requeries;
        self
    }

    /// The decorated instance registered under `name`, if any.
    ///
    /// This is the capability path: a `tool_call` node takes the instance and
    /// invokes it directly, with no middleware in between. It is decorated
    /// because it was decorated before it was stored.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.groups
            .values()
            .find(|tool| tool.name() == name)
            .map(Arc::clone)
    }

    /// The names of the registered tools, in group order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.groups
            .values()
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    /// The **model-facing** schemas, with injected arguments projected out.
    ///
    /// A group the grant withheld is not here, because it is not registered.
    #[must_use]
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.groups
            .values()
            .map(|tool| tool.schema().without(tool.injected_arguments()))
            .collect()
    }

    /// The **declared** schemas, injected arguments included.
    ///
    /// The introspection view. Never put this on the wire; use
    /// [`Self::schemas`].
    #[must_use]
    pub fn declared_schemas(&self) -> Vec<ToolSchema> {
        self.groups.values().map(|tool| tool.schema()).collect()
    }

    /// The failures recorded so far, oldest first.
    #[must_use]
    pub fn history(&self) -> Vec<ToolMessage> {
        lock(&self.history).clone()
    }

    /// Invokes the named tool.
    ///
    /// A failure the tool marks recoverable comes back as a [`ToolOutcome`]
    /// carrying model-readable content and the recovery that was applied, and
    /// is appended to [`Self::history`]. Arguments that did not parse are
    /// handled the same way, without invoking anything: the raw string the
    /// model emitted is fed back so it can try again.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownTool`] when the call names a tool this set does not
    ///   hold — which is what a withheld group looks like from here.
    /// - [`Error::ToolFatal`] when the tool reports [`Recovery::Fatal`], the
    ///   only recovery that ends a step.
    /// - [`Error::RequeriesExhausted`] when one tool's requeries pass the
    ///   bound, so a broken tool cannot spend the whole run being re-asked.
    pub fn invoke(&self, call: &ToolInvocation) -> Result<ToolOutcome> {
        let Some(tool) = self.tool(&call.name) else {
            return Err(Error::UnknownTool {
                name: call.name.clone(),
            });
        };
        if let Some(reason) = &call.invalid {
            let text = format!("tool error: arguments did not parse: {reason}");
            return self.recovered(call, text, Recovery::Requery);
        }
        match tool.invoke(call) {
            Ok(report) => match report.recovery {
                Some(recovery) => self.recovered(call, report.content, recovery),
                None => Ok(ToolOutcome::new(&call.id, &call.name, report.content, None)),
            },
            // `Resilient` lets only a fatal error past, and every instance in
            // this set was wrapped at construction.
            Err(error) => {
                self.record(call, error.model_readable(), Recovery::Fatal);
                Err(Error::ToolFatal {
                    tool: call.name.clone(),
                    message: error.message,
                })
            }
        }
    }

    /// Records a recovered failure and answers with it, bounding requeries.
    fn recovered(
        &self,
        call: &ToolInvocation,
        text: String,
        recovery: Recovery,
    ) -> Result<ToolOutcome> {
        self.record(call, text.clone(), recovery);
        if recovery == Recovery::Requery {
            let mut requeries = lock(&self.requeries);
            let spent = requeries.entry(call.name.clone()).or_insert(0);
            *spent = spent.saturating_add(1);
            if *spent > self.max_requeries {
                return Err(Error::RequeriesExhausted {
                    tool: call.name.clone(),
                    limit: self.max_requeries,
                });
            }
        }
        Ok(ToolOutcome::new(&call.id, &call.name, text, Some(recovery)))
    }

    /// Appends one failure to the history.
    fn record(&self, call: &ToolInvocation, text: String, recovery: Recovery) {
        lock(&self.history).push(ToolMessage {
            call_id: call.id.clone(),
            tool: call.name.clone(),
            text,
            recovery,
        });
    }
}

/// The offline reference tool set: four pure functions, one per verb.
///
/// Pure in the strict sense — no state, no file system, no clock — so the same
/// arguments produce the same result on every run and every bundled example is
/// deterministic with no credentials and no network.
#[derive(Debug, Clone, Copy)]
pub struct PureTools;

impl PureTools {
    /// The four reference instances, undecorated.
    ///
    /// [`ToolSet::from_groups`] decorates them; handing them out undecorated
    /// here is what lets a test prove the decoration is the set's doing.
    #[must_use]
    pub fn groups() -> ToolGroups {
        ToolGroups {
            read: Arc::new(ReadTool),
            search: Arc::new(SearchTool),
            edit: Arc::new(EditTool),
            execute: Arc::new(ExecuteTool),
        }
    }
}

/// The corpus the reference read and search tools answer from.
const CORPUS: [(&str, &str); 3] = [
    ("notes.md", "the loop stops when the judge says so"),
    ("plan.md", "attempt, evaluate, route, budget"),
    (
        "readme.md",
        "a loop is a harness, a memory, tools, a workspace",
    ),
];

/// Reads one document out of the reference corpus.
#[derive(Debug, Clone, Copy)]
struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new("read", "read one document by name", &["path"])
    }

    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        let path = call.argument("path");
        CORPUS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, body)| ToolReport::ok(*body))
            .ok_or_else(|| ToolError::requery(format!("no document named {path}")))
    }
}

/// Finds the documents whose text holds a term.
#[derive(Debug, Clone, Copy)]
struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new("search", "find documents holding a term", &["term"])
    }

    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        let term = call.argument("term");
        if term.is_empty() {
            return Err(ToolError::requery("search needs a term"));
        }
        let hits = CORPUS
            .iter()
            .filter(|(_, body)| body.contains(term))
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        Ok(ToolReport::ok(hits.join(", ")))
    }
}

/// Returns what a document would hold after a replacement.
#[derive(Debug, Clone, Copy)]
struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "edit",
            "replace a fragment of a document",
            &["path", "from", "to"],
        )
    }

    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        let path = call.argument("path");
        let from = call.argument("from");
        let Some((_, body)) = CORPUS.iter().find(|(name, _)| *name == path) else {
            return Err(ToolError::requery(format!("no document named {path}")));
        };
        if !body.contains(from) || from.is_empty() {
            return Err(ToolError::requery(format!("{path} does not hold {from}")));
        }
        Ok(ToolReport::ok(body.replace(from, call.argument("to"))))
    }
}

/// Runs a command inside a sandbox the host names.
///
/// `sandbox` is an injected argument: the host supplies it, the model never
/// sees it in [`ToolSet::schemas`], and a sandbox reported dead salvages a diff
/// out of the trajectory rather than failing the step.
#[derive(Debug, Clone, Copy)]
struct ExecuteTool;

impl Tool for ExecuteTool {
    fn name(&self) -> &str {
        "execute"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "execute",
            "run a command and report what it printed",
            &["command", "trajectory", "sandbox"],
        )
    }

    fn injected_arguments(&self) -> &[&'static str] {
        &["sandbox"]
    }

    fn invoke(&self, call: &ToolInvocation) -> std::result::Result<ToolReport, ToolError> {
        let command = call.argument("command");
        if command.is_empty() {
            return Err(ToolError::requery("execute needs a command"));
        }
        if call.argument("sandbox") == "dead" {
            let trajectory = call.argument("trajectory");
            if trajectory.is_empty() {
                return Err(ToolError::fatal("the sandbox is gone and left nothing"));
            }
            return Err(ToolError::salvaged(
                "the sandbox is gone",
                format!("--- reconstructed\n+++ {trajectory}"),
            ));
        }
        Ok(ToolReport::ok(format!("ran {command}")))
    }
}

#[cfg(test)]
mod test;
