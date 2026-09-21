//! Host-owned roles for a supervised conversation. Models cannot change these roles.

/// Selects the tool boundary of a conversation and its sole execution session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorSession {
    /// The user-facing planner and reviewer, with no environment execution tools.
    Supervisor,
    /// The host-owned Implementer, which reports only to its Supervisor.
    Implementer,
}
