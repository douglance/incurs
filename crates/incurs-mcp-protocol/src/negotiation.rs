//! Client and server standard negotiation.

use std::time::SystemTime;

use thiserror::Error;

use crate::core::{McpLifecycleFamily, McpVersion};
use crate::standards::{known_standard, known_standards};

/// Ordered set of exact MCP standards enabled by a host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpStandardSet {
    versions: Vec<McpVersion>,
}

impl Default for McpStandardSet {
    fn default() -> Self {
        Self::all()
    }
}

impl McpStandardSet {
    /// Enables every official standard, preferring newest first.
    pub fn all() -> Self {
        Self {
            versions: known_standards()
                .iter()
                .rev()
                .map(|standard| McpVersion::from(standard.version()))
                .collect(),
        }
    }

    /// Enables only stateless modern standards.
    pub fn modern_only() -> Self {
        Self::all().filter(McpLifecycleFamily::Modern)
    }

    /// Enables only stateful legacy standards.
    pub fn legacy_only() -> Self {
        Self::all().filter(McpLifecycleFamily::Legacy)
    }

    /// Creates an enabled set in caller-supplied preference order.
    pub fn from_versions(
        versions: impl IntoIterator<Item = McpVersion>,
    ) -> Result<Self, McpNegotiationError> {
        let mut enabled = Vec::new();
        for version in versions {
            if known_standard(&version).is_none() {
                return Err(McpNegotiationError::UnknownStandard(version));
            }
            if !enabled.contains(&version) {
                enabled.push(version);
            }
        }
        if enabled.is_empty() {
            return Err(McpNegotiationError::NoEnabledStandards);
        }
        Ok(Self { versions: enabled })
    }

    /// Returns enabled versions in preference order.
    pub fn versions(&self) -> &[McpVersion] {
        &self.versions
    }

    /// Moves an enabled version to the front of the preference order.
    pub fn with_preference(mut self, preferred: &McpVersion) -> Result<Self, McpNegotiationError> {
        let Some(index) = self
            .versions
            .iter()
            .position(|version| version == preferred)
        else {
            return Err(McpNegotiationError::DisabledStandard(preferred.clone()));
        };
        let version = self.versions.remove(index);
        self.versions.insert(0, version);
        Ok(self)
    }

    fn filter(mut self, lifecycle: McpLifecycleFamily) -> Self {
        self.versions.retain(|version| {
            known_standard(version).is_some_and(|standard| standard.lifecycle() == lifecycle)
        });
        self
    }

    fn preferred_common(
        &self,
        supported: &[McpVersion],
        lifecycle: McpLifecycleFamily,
    ) -> Option<McpVersion> {
        self.versions
            .iter()
            .find(|version| {
                supported.contains(version)
                    && known_standard(version)
                        .is_some_and(|standard| standard.lifecycle() == lifecycle)
            })
            .cloned()
    }

    fn preferred(&self, lifecycle: McpLifecycleFamily) -> Option<McpVersion> {
        self.versions
            .iter()
            .find(|version| {
                known_standard(version).is_some_and(|standard| standard.lifecycle() == lifecycle)
            })
            .cloned()
    }
}

/// Evidence obtained from a modern negotiation probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpNegotiationEvidence {
    /// `server/discover` returned supported exact standards.
    Discovered {
        /// Standards advertised by the server.
        supported: Vec<McpVersion>,
    },
    /// The peer recognized modern negotiation but rejected the requested exact
    /// standard with MCP's unsupported-version error.
    UnsupportedVersion {
        /// Standards advertised in the error.
        supported: Vec<McpVersion>,
    },
    /// A stdio peer did not recognize the modern discovery request.
    StdioUnrecognized,
    /// An HTTP peer returned a 400 response that does not recognize modern MCP.
    HttpBadRequestUnrecognized,
    /// The remote endpoint rejected authentication or authorization.
    AuthenticationFailure,
    /// The remote endpoint failed with a 5xx response.
    ServerFailure,
    /// The transport failed before returning protocol evidence.
    TransportFailure,
    /// The peer returned a malformed or contradictory modern response.
    InvalidModernResponse,
}

/// Negotiation result for the next lifecycle action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpNegotiationDecision {
    /// Use a modern standard discovered by the peer.
    SelectModern(McpVersion),
    /// Retry the recognized modern peer with a mutually supported standard.
    RetryModern(McpVersion),
    /// Restart or continue with legacy initialization.
    FallBackLegacy(McpVersion),
}

/// Negotiation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum McpNegotiationError {
    /// The configured version is not an official standard.
    #[error("unknown MCP standard {0}")]
    UnknownStandard(McpVersion),
    /// The requested preference is not enabled.
    #[error("MCP standard {0} is not enabled")]
    DisabledStandard(McpVersion),
    /// No standards were configured.
    #[error("at least one MCP standard must be enabled")]
    NoEnabledStandards,
    /// Client and server have no mutually supported standard.
    #[error("client and server have no mutually supported MCP standard")]
    NoCommonStandard,
    /// Modern negotiation failed without valid legacy evidence.
    #[error("MCP negotiation failed closed: {0}")]
    FailedClosed(&'static str),
}

/// Selects the next lifecycle action from protocol evidence.
pub fn negotiate(
    enabled: &McpStandardSet,
    evidence: &McpNegotiationEvidence,
) -> Result<McpNegotiationDecision, McpNegotiationError> {
    match evidence {
        McpNegotiationEvidence::Discovered { supported } => enabled
            .preferred_common(supported, McpLifecycleFamily::Modern)
            .map(McpNegotiationDecision::SelectModern)
            .ok_or(McpNegotiationError::NoCommonStandard),
        McpNegotiationEvidence::UnsupportedVersion { supported } => enabled
            .preferred_common(supported, McpLifecycleFamily::Modern)
            .map(McpNegotiationDecision::RetryModern)
            .ok_or(McpNegotiationError::NoCommonStandard),
        McpNegotiationEvidence::StdioUnrecognized
        | McpNegotiationEvidence::HttpBadRequestUnrecognized => enabled
            .preferred(McpLifecycleFamily::Legacy)
            .map(McpNegotiationDecision::FallBackLegacy)
            .ok_or(McpNegotiationError::NoCommonStandard),
        McpNegotiationEvidence::AuthenticationFailure => {
            Err(McpNegotiationError::FailedClosed("authentication failure"))
        }
        McpNegotiationEvidence::ServerFailure => {
            Err(McpNegotiationError::FailedClosed("server failure"))
        }
        McpNegotiationEvidence::TransportFailure => {
            Err(McpNegotiationError::FailedClosed("transport failure"))
        }
        McpNegotiationEvidence::InvalidModernResponse => {
            Err(McpNegotiationError::FailedClosed("invalid modern response"))
        }
    }
}

/// Cache scope for a prior standard negotiation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpNegotiationScope {
    /// One restartable stdio process definition.
    Process(String),
    /// One normalized HTTP origin.
    Origin(String),
}

/// Previously selected immutable standard for one peer scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpPriorNegotiation {
    scope: McpNegotiationScope,
    standard: McpVersion,
    expires_at: Option<SystemTime>,
    failed: bool,
}

impl McpPriorNegotiation {
    /// Creates a prior negotiation.
    pub fn new(
        scope: McpNegotiationScope,
        standard: McpVersion,
        expires_at: Option<SystemTime>,
    ) -> Self {
        Self {
            scope,
            standard,
            expires_at,
            failed: false,
        }
    }

    /// Marks the prior selection as failed so the next use renegotiates once.
    pub fn mark_failed(&mut self) {
        self.failed = true;
    }

    /// Returns a reusable exact standard for the matching scope.
    pub fn reusable(&self, scope: &McpNegotiationScope, now: SystemTime) -> Option<&McpVersion> {
        if self.failed
            || &self.scope != scope
            || self.expires_at.is_some_and(|expiry| expiry <= now)
        {
            None
        } else {
            Some(&self.standard)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn versions(versions: &[&str]) -> Vec<McpVersion> {
        versions
            .iter()
            .map(|version| McpVersion::from(*version))
            .collect()
    }

    #[test]
    fn all_prefers_newest_without_lexical_version_comparison() {
        assert_eq!(
            McpStandardSet::all()
                .versions()
                .iter()
                .map(McpVersion::as_str)
                .collect::<Vec<_>>(),
            [
                "2026-07-28",
                "2025-11-25",
                "2025-06-18",
                "2025-03-26",
                "2024-11-05",
            ]
        );
    }

    #[test]
    fn discovery_selects_preferred_common_modern_standard() {
        assert_eq!(
            negotiate(
                &McpStandardSet::all(),
                &McpNegotiationEvidence::Discovered {
                    supported: versions(&["2025-11-25", "2026-07-28"]),
                },
            ),
            Ok(McpNegotiationDecision::SelectModern(McpVersion::from(
                "2026-07-28"
            )))
        );
    }

    #[test]
    fn only_protocol_specific_evidence_allows_legacy_fallback() {
        let enabled = McpStandardSet::all();
        assert_eq!(
            negotiate(
                &enabled,
                &McpNegotiationEvidence::HttpBadRequestUnrecognized
            ),
            Ok(McpNegotiationDecision::FallBackLegacy(McpVersion::from(
                "2025-11-25"
            )))
        );
        for evidence in [
            McpNegotiationEvidence::AuthenticationFailure,
            McpNegotiationEvidence::ServerFailure,
            McpNegotiationEvidence::TransportFailure,
            McpNegotiationEvidence::InvalidModernResponse,
        ] {
            assert!(matches!(
                negotiate(&enabled, &evidence),
                Err(McpNegotiationError::FailedClosed(_))
            ));
        }
    }

    #[test]
    fn prior_negotiation_is_scoped_expires_and_reprobes_after_failure() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let scope = McpNegotiationScope::Origin("https://example.test".to_string());
        let mut prior = McpPriorNegotiation::new(
            scope.clone(),
            McpVersion::from("2026-07-28"),
            Some(now + Duration::from_secs(60)),
        );
        assert_eq!(
            prior.reusable(&scope, now).map(McpVersion::as_str),
            Some("2026-07-28")
        );
        assert_eq!(
            prior.reusable(
                &McpNegotiationScope::Origin("https://other.test".to_string()),
                now
            ),
            None
        );
        assert_eq!(prior.reusable(&scope, now + Duration::from_secs(60)), None);
        prior.mark_failed();
        assert_eq!(prior.reusable(&scope, now), None);
    }
}
