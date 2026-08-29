//! The orchestrator as a registration, not as a prompt.
//!
//! Every constraint the orchestrator specification names is a property of how
//! this type is *constructed*: the tools it holds, the specialists it may
//! reach, and the fact that neither set has an extend method. That placement is
//! the whole argument. Telling a model "delegate, do not execute" works until
//! executing is the locally cheaper path; removing the capability removes the
//! option, and a spawn naming an undeclared specialist fails loudly rather than
//! falling back to whatever the host happens to have registered.

use std::collections::BTreeSet;

use crate::error::{Error, Result};
use crate::harness::{Brief, Delegate, RoleRegistry, Ticket};
use crate::tools::{ToolGrant, ToolGroup};

/// The specialists an orchestrator may spawn, fixed at construction.
///
/// A declared list rather than "whatever the host registry holds". A registry
/// grows over a project's life; a role's delegate list is a decision somebody
/// made, and the two should be checked against each other
/// ([`Orchestrator::verify_declared_in`]) rather than silently unified.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DelegateSet {
    names: BTreeSet<String>,
}

impl DelegateSet {
    /// Declares exactly these specialists.
    pub fn of<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether `name` is declared.
    #[must_use]
    pub fn holds(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Every declared name, in a stable order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// How many specialists are declared.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether nothing is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The role bound to `plan`, `attempt`, and `report`.
///
/// One role at all three nodes, holding one grant and one delegate set. What
/// differs between the nodes is what they are handed and what they must
/// produce, which is [`steps`](super::steps) rather than this type.
///
/// # The two rules this type *is*
///
/// **It holds no execution tools.** [`ToolGroup::Edit`] and
/// [`ToolGroup::Execute`] are refused at construction, naming the offender.
/// Reading and searching are left, because an orchestrator that cannot read
/// what its specialists wrote cannot compose a report from it.
///
/// **Its delegates are a closed set.** [`Self::spawn`] checks the name against
/// the declared set *before* it reaches the [`Delegate`], so an undeclared
/// specialist is an error rather than a lookup in somebody else's registry.
///
/// # Examples
///
/// ```
/// # use tinyloops::{DelegateSet, Orchestrator, ToolGrant, ToolGroup};
/// let orchestrator = Orchestrator::new(
///     ToolGrant::read_only(),
///     DelegateSet::of(["prover", "refuter"]),
/// )?;
///
/// assert!(orchestrator.may_spawn("prover"));
/// assert!(!orchestrator.may_spawn("shell-runner"));
/// # Ok::<(), tinyloops::Error>(())
/// ```
///
/// A grant that can execute is refused, and the error names the group:
///
/// ```
/// # use tinyloops::{DelegateSet, Error, Orchestrator, ToolGrant, ToolGroup};
/// let refused = Orchestrator::new(
///     ToolGrant::of(&[ToolGroup::Read, ToolGroup::Execute]),
///     DelegateSet::of(["prover"]),
/// );
///
/// assert!(matches!(
///     refused,
///     Err(Error::ExecutionToolInOrchestrator { group: "execute" })
/// ));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orchestrator {
    grant: ToolGrant,
    delegates: DelegateSet,
}

impl Orchestrator {
    /// The groups an orchestrator may never hold, in the order they are checked.
    ///
    /// Editing a program and running one are the two ways a driver stops
    /// commissioning work and starts doing it. Both are named here rather than
    /// inferred from a tool's name, because a grant is a set of groups and a
    /// group is the unit a registration actually decides.
    pub const FORBIDDEN: [ToolGroup; 2] = [ToolGroup::Edit, ToolGroup::Execute];

    /// Registers an orchestrator.
    ///
    /// Both sets are fixed here and there is deliberately no method that
    /// extends either. A capability acquired after construction is a capability
    /// nobody reviewed.
    ///
    /// # Errors
    ///
    /// - [`Error::ExecutionToolInOrchestrator`] when `tools` holds an editing
    ///   or executing group, naming which.
    /// - [`Error::EmptyDelegateSet`] when `delegates` is empty. Holding no
    ///   execution tools and no specialists leaves a role that can neither act
    ///   nor commission action.
    pub fn new(tools: ToolGrant, delegates: DelegateSet) -> Result<Self> {
        for group in Self::FORBIDDEN {
            if tools.holds(group) {
                return Err(Error::ExecutionToolInOrchestrator {
                    group: group.as_str(),
                });
            }
        }
        if delegates.is_empty() {
            return Err(Error::EmptyDelegateSet);
        }
        Ok(Self {
            grant: tools,
            delegates,
        })
    }

    /// The tools it holds.
    #[must_use]
    pub fn grant(&self) -> &ToolGrant {
        &self.grant
    }

    /// The specialists it may reach.
    #[must_use]
    pub fn delegates(&self) -> &DelegateSet {
        &self.delegates
    }

    /// Whether `role` is one of them.
    #[must_use]
    pub fn may_spawn(&self, role: &str) -> bool {
        self.delegates.holds(role)
    }

    /// Starts `role` on `brief`, through `delegate`, and returns immediately.
    ///
    /// The declared-set check happens here rather than inside the [`Delegate`],
    /// so an undeclared name never reaches the harness at all. That ordering is
    /// the point: a harness asked for a name it does not know would report an
    /// unknown role, which reads like a configuration slip rather than like the
    /// orchestrator overstepping its registration.
    ///
    /// # Errors
    ///
    /// - [`Error::UndeclaredDelegate`] when `role` is outside the declared set.
    /// - Whatever the [`Delegate`] raises once the name is accepted.
    pub fn spawn(&self, delegate: &dyn Delegate, role: &str, brief: Brief) -> Result<Ticket> {
        if !self.may_spawn(role) {
            return Err(Error::UndeclaredDelegate {
                name: role.to_owned(),
            });
        }
        delegate.spawn(role, brief)
    }

    /// Asserts every declared specialist exists in `registry`.
    ///
    /// A role and a registry that disagree fail here, at wiring time, rather
    /// than on the pass that first needs the missing specialist.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRole`] naming the first declared delegate the registry
    /// does not hold.
    pub fn verify_declared_in(&self, registry: &RoleRegistry) -> Result<()> {
        for name in self.delegates.names() {
            registry.resolve(name)?;
        }
        Ok(())
    }
}
