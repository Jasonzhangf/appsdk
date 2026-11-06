# Collab verification map

This file maps current behavior to its source and isolated public consumers.
It does not assert that a candidate is installed, activated or accepted live.
Execution results belong in the task run. Graph validation proves topology and
operator bindings; it does not prove runtime behavior.

## Current identity and storage

`identity-map.json` is the only identity-selection resource. Current tasks,
messages, subscriptions and real receive receipts are persisted in `state.json`.
Route indexes and host observations serve their declared owners. No journal,
operation history, endpoint generation or old tmux display name selects an
identity. Opaque legacy files are not replayed.

| Behavior | Current gates | Required boundary |
|---|---|---|
| Register, recover, rename, rebind and change role through the current map | `context_operation_public_contract`; `server::binding_store_tests`; `server::name_resolution_tests` | Current authorization is applied through one owner. Missing registration facts and genuine file errors remain explicit. Historical records cannot veto a new authorized binding. |
| Fixed TUI placement and physical name | `register_reply_public_contract`; `tmux_master_authority_cli`; `current_placement`; Context public rename controls | Use `<cwd-folder>_<slot>_<name>`. The fixed slot is distinct from tmux `$N` and the agent sessionID. Jenny/master uses slot 1. The shared layout manifest owns `tab_id` and `pane_order`; display text is not identity authority. |
| Prepare rename before publishing a new or refreshed binding | Context first-register/refresh rename-conflict controls; scoped-register tests | An unpublished failure preserves map, credentials, business state and both physical names. Published durability-unknown retains published facts. Bounded name effects preserve primary-before-outgoing ordering. |
| Codex Desktop registration and continuation | `desktop_bridge_public_contract`; `desktop-runtime.test.mjs`; `desktop-bridge.test.mjs` | Use the real native thread/session and Desktop transport. No tmux facts are required. Publisher admission verifies the exact native target. |
| DSH registration and continuation | `dsh_master_authority_cli`; `server::dsh_channel_tests` | Use the real gateway session and enrolled DSH binding. UUID adoption alone does not prove communication. Gateway polling remains the DSH consumption path. |
| AGY TUI registration | `register_reply_public_contract::agy_tmux_with_inherited_cross_project_codex_registers_real_agy_session_and_subsequent_commands_agree` | Persist the actual AGY session and host type. Inherited Codex facts cannot change the selected host. |
| Current-file startup and storage boundary | `server::startup_tests`; scope tests; `appserver_two_tui_integration::degraded_user_maintenance`; Desktop Node consumer | Restore current files. Reject malformed current state before publishing a listener. Validate storage ownership before creating a namespace. Keep foreign resources and opaque legacy bytes unchanged. |
| Read-only projection and endpoint observation | Context query controls; `identity_endpoint_projection`; `master_status_cli`; `tmux_recv_e2e` | Query does not register, consume messages or perform communication recovery. Current reachability is distinct from stable identity and task ownership. |

The affected graphs are `collab-identity-anchor`, `collab-context`,
`collab-binding-lifecycle`, `collab-name-ownership`, `collab-startup-recovery`
and `collab-work-role-registration` under `docs/dagpipe/`.

## Current work and messages

| Behavior | Current gates | Required boundary |
|---|---|---|
| Same-name session continuation and explicit task reassignment | AppServer integration `name_runtime_cli`, `name_adjudication`, `board_cli`; board name/reassignment units | Update the carrier for unclosed work owned by that name. Preserve task ID, lifecycle status, evidence, resources and integration/cleanup obligations. Closed task records retain their original carrier. |
| Current-name unread mailbox handoff | `server::name_mailbox_tests`; `name_mailbox_public_repeated_release_reallocation_keeps_current_carrier`; `name_mailbox_public_distinct_names_follow_address_after_release` | The current project/name address is sufficient even after vacancy. Preserve message ID, body, sender and real consumption receipts. Other names and consumed messages do not move. Unchanged carriers do not create a new wake. |
| First peer registration notice | AppServer integration `work_roles_cli::minimal_registration_*` | Publish eligible worker registration, default subscription and advisory message in the same current-state business transaction. No self/master/backfill notice; repeat Context/Register/restart stays silent. A rejected native wake retains durable mail. |
| Roles, templates and work policy | AppServer integration `work_roles_cli`, `workflow_templates`; work assignment and required-review units | Role changes and template changes notify only affected counterparts. Pinned executor/reviewer and accepted-task obligations retain their existing semantics. |
| Board lifecycle and capacity | AppServer integration `board_cli`; `server::board_state_tests`; `server::board_reassign_tests`; `server::scheduler_admission_tests` | Claim/invite/respond, delivery, review, integration and cleanup use current persisted state. Closed tasks disappear from open views. Ordinary peers require Board admission for task dispatch; ordinary messages have no role-pair restriction. |
| Cross-project ordinary messaging | `cross_project_stale_identity_cli` | Any registered role pair, including peer/CEO, can send directly. Preserve project isolation, exact address resolution, authenticated source and correlated reply routing. |
| Delivery versus consumption | `appserver_two_tui_integration`; `desktop_bridge_public_contract`; `dsh_master_authority_cli`; notification batch units | Durable enqueue, native acceptance, actual receive receipt and reply are separate facts. Context/inbox/status do not consume. Unknown native outcomes do not trigger blind resubmission or transport switching. |
| Subscription and patrol persistence | AppServer integration `patrol_runtime`, `patrol_subscriptions`, `project_patrol_subscribe`, `ceo_patrol_runtime`; `goal_periodic_blackbox`; timer units | Preserve the current subscription and finite schedule. Explicit periodic declaration controls repetition; ordinary Context invents no schedule. Cancelled patrols do not remove direct-message reception. |

`collab-send-message`, `collab-board-assignment-lifecycle`,
`collab-dsh-channel`, `collab-dsh-delivery` and the notification-consumption
graph bind these owners. DSH first registration, delivery acceptance,
consumption and reply have distinct endpoints; they are not one graph with a
recovery back edge.

## Explicit current communication recovery

`appserver_two_tui_integration::manual_communication` forwards actual accepted
adapter input to an independent receiving test process. The receiver executes
real CLI `recv`, then sends an ordinary authenticated correlated self-reply.
The initiating process does not consume, acknowledge or reply on its behalf.

`desktop_manual_recovery_*` uses the Desktop native bridge with no tmux.
`dsh_manual_recovery_*` uses actual enrolled gateway enqueue and a separate
authenticated canonical Poll/reply; CLI `recv` remains forbidden for DSH.

The selected binding of the message's latest actual delivery attempt is stored
in `notification_delivery_bindings` and removed with that message. It is not an
operation ledger. Explicit recovery sends only for a known broken current
binding. Acceptance without receiver consumption stays pending. Recovery
requires the exact binding, real receive receipt and authenticated P-to-P reply
with `in_reply_to=M`. Healthy and unknown paths do not send. Query, ordinary
Context and restart stay silent. Repeated explicit calls reuse the current
pending exchange. A changed binding or another peer's legal reply cannot
satisfy that exchange. Lost or undecodable native responses stay unknown.

The owner is `server/identity_context/communication.rs`, reached only through
mapped Context. The Context graph includes the projection, ordinary self-Send
and unified result; it does not add an automatic retry loop or product timeout.

## Lifecycle and authority

| Behavior | Current gates | Required boundary |
|---|---|---|
| Global CEO and approved master bootstrap | `ceo_public_contract`; CEO state/report/idle tests | Explicit typed global grant, exact source and scope, current target and a real registered-master handshake. Bootstrap follows the approved stable session/pane, even if registration changes its physical label. |
| Peer lifecycle and self removal | `peer_lifecycle_cli`; `peer_lifecycle_missing_close_cli`; `self_close_cross_project_cli`; `lifecycle_boundary_cli` | Missing exact pane permits retirement. Present or unknown panes do not. Self removal preserves host process, outstanding task/message data and foreign identity; another project's later Context does not inherit retired anchors. |
| Scoped project reset | `lifecycle_boundary_cli`; `server::project_reset::tests` | Authorize without recovery side effects. Delete only the requested project's current map/state/routes/assets. Failed cleanup remains explicit and pending; other projects and the global daemon remain. |
| User-managed activation | `lifecycle_boundary_cli`; autostart tests; `collab-activation.graph.json` | Agent requests do not start or replace the production daemon. Formal user activation waits for unloaded service state before bootstrap and returns real failure. |
| Authenticated wire Shutdown | `appserver_two_tui_integration::degraded_user_maintenance::current_daemon_maintenance_is_authenticated_and_malformed_state_fails_startup`; `collab-shutdown.graph.json` | Reject absent/wrong credentials. Write the complete authorized response, cancel blocked response writes on stop, exit cleanly with an unread client present, and remove the owned socket/PID file. This isolated test is not production lifecycle authorization. |

Collab does not own the tmux server lifecycle. Test cleanup retires only the
test's exact owned session using its stable target after rename. No installation,
registration, identity recovery or daemon maintenance requires terminating a
tmux server or host pane.

## QQBot typed channel boundary

- `qqbot_communication_cli` covers current/legacy send syntax, explicit content,
  list/default parsing and missing conversation context.
- AppServer integration `channel_inbound_cli` covers typed envelope publication,
  platform-event deduplication and reply routing through `in_reply_to`.
- QQBot lifecycle, channel envelope and adapter units cover conversation/channel
  default isolation across current-state restart, exact leading address handling,
  typed CEO resolution, inbound save-before-publish and attachment collisions.
- Outbound files must be explicit typed attachments inside the authorized root.
  A body path is not read. Upload failures and uncertain outcomes remain explicit.
- `qqbot-receiver-routing.test.mjs` covers trailing-window batching, in-flight
  attachment completion and draining queued batches on shutdown.

The QQBot inbound/outbound graphs describe these source contracts. Real platform
upload receipts and live external delivery require separate live evidence.
