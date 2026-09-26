pub mod global_state;
pub(crate) mod keepalive;
pub mod mailbox;
pub mod notification_contract;
pub mod notification_state;
pub mod presence;
pub mod state;
pub mod timers;

pub const EXPLICIT_UNSUBSCRIBE_REASON: &str = "explicit-unsubscribe";

const NOTIFICATION_SUBSCRIPTION_MISSING_ERROR: &str =
    "no armed direct-message subscription matches this recipient's registered transport";
const MAILBOX_ONLY_ESCALATION: &str = "the message is durable but this wake has no subscription; have the recipient run collab context to recover or re-register its default direct-message lease, then send a new message if another wake is needed";

pub use global_state::{GlobalState, ProjectRegistration, RuntimeBinding};

use crate::identity::{
    AgentId, AppServerId, BindingId, CommandId, NativeThreadId, OperationId, RuntimeId,
};
use crate::proto::{
    CommandEnvelope, ProjectContext, Req, RequestEnvelope, Resp, RouteResolution,
    SelectedTransport, TransportCandidates, TransportKind, MSG_TYPES,
};
use crate::scope::{HostPaths, ProjectScopeId, RouteScope, Scope};
use crate::server::presence::{append_log, IdentityPresence};
use mailbox::{
    batch_notification_text, compose_notification, default_direct_message_id,
    is_explicit_notification, missing_recipient_projection_messages, notification_text,
    read_recipient_mailbox, truncate_notification, DEFAULT_DIRECT_MESSAGE_TTL_SECONDS,
    MAX_ACTIVE_SUBSCRIPTIONS_PER_WORKER, MAX_NOTIFICATION_TTL_SECONDS, NOTIFICATION_EVENTS,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use state::{
    goal_deadline_key, now_ms, task_resource_active, wait_cycle, CleanupReceipt,
    CleanupVerification, Event, GlobalEvent, Message, MigrationRecord, NotificationSubscription,
    State, TaskRec, TypedCommand, TypedEnvelope, WaitSpec, WorkerRec, WorktreeBinding,
    MAX_WAKE_ATTEMPTS,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::net::UnixListener;
use tokio::sync::Notify;

#[cfg(test)]
#[path = "notification_batch_tests.rs"]
mod notification_batch_tests;

#[cfg(test)]
#[path = "host_route_registry_tests.rs"]
mod host_route_registry_tests;

#[cfg(test)]
#[path = "reducer_binding_tests.rs"]
mod reducer_binding_tests;

#[cfg(test)]
#[path = "startup_tests.rs"]
mod startup_tests;

#[cfg(test)]
#[path = "scheduler_admission_tests.rs"]
mod scheduler_admission_tests;

#[cfg(test)]
pub(crate) mod peer_tests;
include!("mod_parts/part_01.rs");
include!("mod_parts/part_02.rs");
include!("mod_parts/part_03.rs");
include!("mod_parts/part_04.rs");
include!("mod_parts/part_05.rs");
include!("mod_parts/part_06.rs");
include!("mod_parts/part_07.rs");
include!("mod_parts/part_08.rs");
include!("mod_parts/part_09.rs");
include!("mod_parts/part_10.rs");
include!("mod_parts/part_11.rs");
include!("mod_parts/part_12.rs");
