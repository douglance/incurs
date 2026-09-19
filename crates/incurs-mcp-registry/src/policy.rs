//! Approval policy for servers the developer already configured.

use incurs_codemode::{ReplayPolicy, ToolAnnotations, ToolOrigin, ToolPolicy, ToolPolicyResolver};

/// How much a remote server's own annotations are trusted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// A remote tool that declares itself read-only, non-destructive, and
    /// closed-world runs without pausing. Everything else pauses.
    #[default]
    AnnotatedReads,
    /// Every remote tool pauses, matching the Code Mode default.
    Strict,
    /// No tool pauses. Never a default.
    Trusted,
}

/// Resolves approval and replay policy for discovered MCP servers.
///
/// Code Mode's own default requires approval for every remote tool, which is
/// correct for an arbitrary server and fatal here: a pause ends the pass, so a
/// program touching three servers costs three model round trips plus three
/// replays, which is exactly the cost this whole system exists to remove.
///
/// The trust argument is narrow and specific. Every server reached this point by
/// being installed into an agent's own configuration by this developer, so its
/// annotations are no less trustworthy here than in the agent that wrote them.
/// That justifies believing a *negative* claim — "this tool reads and does not
/// reach outside" — and nothing more. The absence of a claim is still treated as
/// unsafe, so an unannotated tool pauses.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoveredToolPolicyResolver {
    mode: ApprovalMode,
}

impl DiscoveredToolPolicyResolver {
    /// Creates a resolver in one mode.
    #[must_use]
    pub fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }
}

impl ToolPolicyResolver for DiscoveredToolPolicyResolver {
    fn resolve(&self, _origin: ToolOrigin, annotations: &ToolAnnotations) -> ToolPolicy {
        let safe_read = annotations.read_only == Some(true)
            && annotations.destructive != Some(true)
            && annotations.open_world != Some(true);
        let requires_approval = match self.mode {
            ApprovalMode::Strict => true,
            ApprovalMode::Trusted => false,
            ApprovalMode::AnnotatedReads => !safe_read,
        };
        ToolPolicy {
            requires_approval,
            // Replay follows effect, not approval: only a closed-world read is
            // safe to run again during a replay pass. Code Mode also rejects a
            // tool that both requires approval and re-executes.
            replay: if safe_read && !requires_approval && annotations.idempotent != Some(false) {
                ReplayPolicy::Reexecute
            } else {
                ReplayPolicy::Log
            },
        }
    }
}
