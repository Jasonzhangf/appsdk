//! Selected transports and the shared endpoint, capability and error contracts
//! consumed by the production adapters.

pub mod codex_app_server;
pub mod desktop_bridge;
pub mod dsh;
pub mod qqbot;
pub mod rex;
pub mod tmux;

pub use crate::proto::RexEndpoint as RexAdapterEndpoint;
pub use codex_app_server::verify_candidate;
pub use rex::{
    RexError, RexEvent, RexEventBatch, RexEventStream, RexInputStage, RexObservation,
    RexProcessObservation, RexProgramStatusOwner, RexProgramStatusRecord, RexWriteReceipt,
    NOT_DELIVERED_PREFIX as REX_NOT_DELIVERED_PREFIX,
};

use serde::{Deserialize, Serialize};
use std::fmt;

pub const APPSERVER_ENV: &str = "COLLAB_APPSERVER";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    Tui,
    Desktop,
}

impl EndpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AdapterError> {
        match value.to_ascii_lowercase().as_str() {
            "tui" | "tui-default" => Ok(Self::Tui),
            "desktop" | "desktop-appserver" => Ok(Self::Desktop),
            _ => Err(AdapterError::UnknownEndpoint {
                observed: Some(value.to_owned()),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeMode {
    None,
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub endpoint: EndpointKind,
    pub submit: bool,
    pub interrupt: bool,
    pub wake: WakeMode,
}

impl AdapterCapabilities {
    /// Describe the endpoint contract independently of its live connection.
    pub fn for_endpoint(endpoint: EndpointKind) -> Self {
        match endpoint {
            EndpointKind::Tui => Self {
                endpoint,
                submit: true,
                interrupt: true,
                wake: WakeMode::Native,
            },
            EndpointKind::Desktop => Self {
                endpoint,
                submit: true,
                interrupt: true,
                wake: WakeMode::Native,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    UnknownEndpoint {
        observed: Option<String>,
    },
    EndpointUnavailable {
        endpoint: EndpointKind,
    },
    RouteUnavailable {
        detail: String,
    },
    /// The App Server refused to load the thread because another live writer
    /// already owns its rollout. This endpoint cannot take over that writer.
    ThreadWriterConflict {
        detail: String,
    },
    CapabilityUnavailable {
        endpoint: EndpointKind,
        operation: &'static str,
    },
    InvalidBinding {
        detail: String,
    },
    WrongTurn {
        expected: Option<String>,
        observed: Option<String>,
    },
    Timeout {
        operation: &'static str,
    },
    Unknown {
        operation: &'static str,
        detail: String,
    },
}

/// How far a failed App Server notification can be known to have progressed.
/// Unknown or accepted native outcomes cannot be resent silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryClass {
    KnownNotDelivered,
    Unknown,
    KnownDelivered,
}

impl AdapterError {
    pub fn notification_delivery_class(&self) -> NotificationDeliveryClass {
        match self {
            Self::RouteUnavailable { .. }
            | Self::EndpointUnavailable { .. }
            | Self::UnknownEndpoint { .. }
            | Self::CapabilityUnavailable { .. }
            | Self::InvalidBinding { .. }
            | Self::WrongTurn { .. } => NotificationDeliveryClass::KnownNotDelivered,
            Self::Timeout { .. } | Self::ThreadWriterConflict { .. } | Self::Unknown { .. } => {
                NotificationDeliveryClass::Unknown
            }
        }
    }
}

const KNOWN_NOT_DELIVERED_PREFIXES: [&str; 11] = [
    desktop_bridge::NOT_SUBMITTED_PREFIX,
    "ADAPTER_ROUTE_UNAVAILABLE:",
    "ADAPTER_ENDPOINT_UNAVAILABLE:",
    "ADAPTER_UNKNOWN_ENDPOINT:",
    "ADAPTER_CAPABILITY_UNAVAILABLE:",
    "ADAPTER_INVALID_BINDING:",
    "ADAPTER_STALE_BINDING:",
    "ADAPTER_WRONG_TURN:",
    // A gateway timeout or malformed reply leaves its enqueue outcome unknown.
    "DSH_ENDPOINT_REJECTED:",
    "DSH_ENDPOINT_BLOCKED:",
    rex::NOT_DELIVERED_PREFIX,
];

/// Classify a sink error by the exact AdapterError display contract.
impl AdapterError {
    pub fn notification_class_from_display(error: &str) -> NotificationDeliveryClass {
        if error
            .starts_with("explicit App Server notification requires the sender native thread id")
        {
            return NotificationDeliveryClass::KnownNotDelivered;
        }
        if error.starts_with("payload is missing result")
            || error.starts_with("ADAPTER_UNKNOWN: decode")
        {
            return NotificationDeliveryClass::Unknown;
        }
        if KNOWN_NOT_DELIVERED_PREFIXES
            .iter()
            .any(|prefix| error.starts_with(prefix))
        {
            return NotificationDeliveryClass::KnownNotDelivered;
        }
        NotificationDeliveryClass::Unknown
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEndpoint { observed } => write!(
                f,
                "ADAPTER_UNKNOWN_ENDPOINT: no registered AppServer endpoint was detected{}",
                observed
                    .as_deref()
                    .map(|value| format!(" (observed {value})"))
                    .unwrap_or_default()
            ),
            Self::EndpointUnavailable { endpoint } => write!(
                f,
                "ADAPTER_ENDPOINT_UNAVAILABLE: {} AppServer endpoint has no live native transport",
                endpoint.as_str()
            ),
            Self::RouteUnavailable { detail } => {
                write!(f, "ADAPTER_ROUTE_UNAVAILABLE: {detail}")
            }
            Self::ThreadWriterConflict { detail } => write!(
                f,
                "APPSERVER_THREAD_WRITER_CONFLICT: {detail} (the recipient thread is owned by another live App Server client; this endpoint can only observe it. Deliver through the App Server instance that owns the thread, or wait for the owning process to exit)"
            ),
            Self::CapabilityUnavailable {
                endpoint,
                operation,
            } => write!(
                f,
                "ADAPTER_CAPABILITY_UNAVAILABLE: {} endpoint does not support {operation}",
                endpoint.as_str()
            ),
            Self::InvalidBinding { detail } => {
                write!(f, "ADAPTER_INVALID_BINDING: {detail}")
            }
            Self::WrongTurn { expected, observed } => write!(
                f,
                "ADAPTER_WRONG_TURN: expected turn {}, observed {}",
                expected.as_deref().unwrap_or("none"),
                observed.as_deref().unwrap_or("none")
            ),
            Self::Timeout { operation } => {
                write!(f, "ADAPTER_TIMEOUT: {operation} timed out")
            }
            Self::Unknown { operation, detail } => {
                write!(f, "ADAPTER_UNKNOWN: {operation} unknown: {detail}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_and_desktop_endpoints_have_distinct_capability_surfaces() {
        let tui = AdapterCapabilities::for_endpoint(EndpointKind::Tui);
        let desktop = AdapterCapabilities::for_endpoint(EndpointKind::Desktop);

        assert_eq!(tui.endpoint, EndpointKind::Tui);
        assert_eq!(tui.wake, WakeMode::Native);
        assert_eq!(desktop.endpoint, EndpointKind::Desktop);
        assert_eq!(desktop.wake, WakeMode::Native);
        assert!(tui.submit && desktop.submit);
        assert!(tui.interrupt && desktop.interrupt);
    }

    #[test]
    fn unknown_endpoint_is_rejected() {
        let error = EndpointKind::parse("unknown").unwrap_err();
        assert!(error.to_string().contains("ADAPTER_UNKNOWN_ENDPOINT"));
    }

    #[test]
    fn timeout_and_unknown_are_explicit_typed_adapter_errors() {
        let timeout_error = AdapterError::Timeout {
            operation: "submit",
        };
        assert!(timeout_error.to_string().starts_with("ADAPTER_TIMEOUT:"));

        let unknown_error = AdapterError::Unknown {
            operation: "interrupt",
            detail: "cursor lost".into(),
        };
        assert!(unknown_error.to_string().starts_with("ADAPTER_UNKNOWN:"));
    }
}
