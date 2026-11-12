# Collab verification map

This file maps current behavior to its source and isolated public consumers.
It does not assert that a candidate is installed, activated or accepted live.
Execution results belong in the task run. Graph validation proves topology and
operator bindings; it does not prove runtime behavior.

The old CLI `migrate`, MCP `collab_migrate` and Migration/RemoveWorker wire
variants are absent. `removed_migration_surface` checks real CLI/MCP rejection
without state creation; the Req decoder's removed-maintenance test pairs unknown
variants with a valid Ping. The current-state public restart case injects unused
old inner migration fields and proves they neither freeze admission nor survive
the next ordinary save. The strict outer reader is unchanged. Scoped reset,
startup, reset intent, retention/timer boundaries and current block approval
retain their dedicated gates. No snapshot/replay maintenance owner remains.

## Current identity and storage

`identity-map.json` is the only identity-selection resource. Current tasks,
messages, subscriptions and real receive receipts are persisted in `state.json`.
Route indexes and host observations serve their declared owners. No journal,
operation history, endpoint generation or old tmux display name selects an
identity. Opaque legacy files are not replayed.

| Behavior | Current gates | Required boundary |
|---|---|---|
| Register, recover, rename, rebind and change role through the current map | `context_operation_public_contract`; AppServer `name_confirmed_takeover`; `server::binding_store_tests`; scoped Register controls | Existing typed business validation precedes credential, rename and active publication. The existing identity gate serializes raw Register through finalization. Confirm the exact new credential before active publication. Active current with an exact valid credential reconstructs an established principal's missing business worker through the shared Register transaction without new onboarding. An initial pending Register completes its original first business transaction once. Query remains pure. Missing facts and genuine file errors remain explicit. |
| Fixed TUI placement and physical name | `context_operation_public_contract` first/nonfirst/duplicate project-layout controls; `register_reply_public_contract`; `tmux_master_authority_cli`; `current_placement` | Project records own fixed placement; overview entries are display references. Persisted project IDs survive mutable names; overview IDs resolve only to a unique current-project record. Duplicate project associations remain explicit errors. Use fixed `tab_id`/`pane_order`; Jenny/master uses project slot 1. Neither tmux `$N`, agent sessionID nor display suffix determines position. |
| Prepare rename before publishing a new or refreshed binding | Context first-register/refresh rename-conflict controls; scoped-register tests | An unpublished failure preserves map, credentials, business state and both physical names. Published durability-unknown retains published facts. Bounded name effects preserve primary-before-outgoing ordering. |
| Codex Desktop registration and continuation | `desktop_bridge_public_contract`; `desktop-runtime.test.mjs`; `desktop-bridge.test.mjs` | Use the real native thread/session and Desktop transport. No tmux facts are required. Publisher admission verifies the exact native target. |
| DSH registration and continuation | `dsh_master_authority_cli`, including absent/returning-binding Context; `server::dsh_channel_tests` | Use the real gateway session and enrolled DSH binding. Scope missing-worker materialization under the identity gate, release before DSH convergence, and reacquire with current state for Context completion. DSH mutation owners acquire their own gate. UUID adoption alone does not prove communication. Gateway polling remains the DSH consumption path. |
| Rex registration, notification and continuation | `rex_public_contract` with an explicit `COLLAB_REX_TEST_SERVER` and `--ignored`; `identity_tests::rex_registration_uses_an_agent_session_and_a_separate_terminal_endpoint`; `server::identity_context::required_fields_tests` | Keep the agent session separate from the Rex server/session/block endpoint. Use the official Rex CLI, an explicit server and autostart disabled. A queued write and accepted Enter do not consume mail. The public consumer checks formal recv, receive-id replay, correlated reply and isolated daemon restart. Reusing the existing explicit-server consumer does not prove ordinary Context discovery or live LLM consumption. |
| Ordinary Rex context discovery | `context_operation_public_contract`; `rex_public_contract`; real Rex LLM agent acceptance | Required/unverified until the public gate and real-agent path pass. Without `--provide`, read `REX_SESSION` and `REX_BLOCK`, obtain the real server from the official Rex CLI, and submit one typed Rex candidate with the independent agent sessionID. Explicit `--provide` Rex facts override environment discovery. Host selection validates only the selected transport's required fields; missing or invalid Rex facts stop before binding publication and never fall through to Desktop discovery. The installed 0.2.0457 `DESKTOP_BRIDGE_THREAD_REQUIRED` result is a red counterexample, not a current-time green claim. |
| Name-pool exhaustion and generated names | `appserver_two_tui_integration::context_names_full_pool_reuses_the_released_source_or_allocates_generated_name`; `context_recovery_pool_overflow_registers_generated_names`; `context_recovery_first_restore_master_with_full_pool_is_not_blocked`; `context_recovery_missing_peer_map_row_does_not_block_current_context` | A full fixed pool does not block registration or authorized takeover. Allocation reuses the same worker's name, then a free fixed name, then the canonical generated name selected by the identity owner. Generated names use the same display-slot validation, control commit, pending effects and task/mailbox ownership as fixed names. A registered peer that lost its map row remains visible as `identity_status: "unmapped"` with null display name and role, while its open tasks, resource task IDs and active subscription IDs stay visible; consumed or terminal subscription history stays hidden. It does not block another peer's current Context or Board read. |
| Endpointless Desktop stale recovery | `peer_lifecycle_cli`; `peer_lifecycle_missing_close_cli`; required/unverified stale identity maintenance controls | Required/unverified. `PEER_LIFECYCLE_TARGET_UNPROVEN` remains the explicit close result whenever absence cannot be proven. Only a Desktop publisher typed runtime descriptor or native `read_thread` proof of exact session/thread absence permits retiring the current execution relation and anchor. Retirement preserves peer identity, name, tasks, unread messages and reply obligations. A later ordinary Context may bind the same name to the current Rex carrier without manual `--provide`; this remains unverified until the public controls pass. |
| Dispatched-attempt timed stale | Required/unverified stale identity maintenance controls; late-event and restart-boundary cases | Timed stale is a distinct branch limited to a dispatched attempt with sustained no perception and no response under key `(attempt_id, execution_generation, anchor_kind, anchor_id)`. It does not apply to ordinary idle, busy, unknown, one failed probe or endpointless close proof failure. Accepted or unknown attempts keep their original facts and are not automatically safe to re-execute; retirement only invalidates the current execution relation and anchor while identity and name remain. |
| Rex activity observation | `server::rex_monitor`; `rex_public_contract` OSC7501 user path | Project working and blocked/question into the existing activity owner. Preserve unknown when records are absent, contradictory, incomplete or disconnected. Activity does not establish presence or consumption. Retire only owned event streams when bindings change or the daemon exits. |
| Machine address and independent daemon delivery | `federation_public_contract`; `address::tests`; `federation::tests`; `server::federation_semantics_tests` | Each daemon owns its identity and mailbox. The public consumer uses private SSH configuration, qualified send, formal recv, reply, operation recovery, name handoff and isolated restart. Loopback SSH evidence and physical-machine evidence have separate scopes. |
| Original QQ channel reply | `federation_public_contract::federation_channel_reply_reaches_the_original_owner_once`; `server::channel_delivery`; `client::adapters::qqbot`; `collab/scripts/tests/qqbot-send.test.mjs` | Keep the original envelope at its owner. Commit the reply before a single persistent sending claim. Invoke the configured helper outside locks, preserve uncertain outcomes and expose platform results separately from mailbox consumption. The public sink test and real QQ platform acceptance are separate evidence. |
| AGY TUI registration | `register_reply_public_contract::agy_tmux_with_inherited_cross_project_codex_registers_real_agy_session_and_subsequent_commands_agree` | Persist the actual AGY session and host type. Inherited Codex facts cannot change the selected host. |
| Current-file startup and storage boundary | `server::startup_tests`; scope tests; `appserver_two_tui_integration::degraded_user_maintenance`; Desktop Node consumer | Restore current files. Reject malformed current state before publishing a listener. Validate storage ownership before creating a namespace. Keep foreign resources and opaque legacy bytes unchanged. |
| Read-only projection and endpoint observation | Context query controls; `identity_endpoint_projection`; `master_status_cli`; `tmux_recv_e2e`; `server::scheduler_admission_tests::idle_capacity_projection_filters_execution_and_observation`; `server::scheduler_admission_tests::idle_capacity_projection_filters_subagents_and_invitations`; `appserver_two_tui_integration::idle_capacity_projection_public_board_and_context_share_pruned_capacity` | Query does not register, consume messages or perform communication recovery. Current reachability is distinct from stable identity and task ownership. Board and Context project idle capacity through one shared pruning owner, keeping only present workers and subagents whose latest observation is idle and that hold no execution reservation; unresponsive history survives. The projection is read-only: it clones the accumulator, does not take a second reducer lock, does not commit `MasterWakeUpdated` and does not mutate journal, task, assignment or notification consumption. Only `master status` keeps the persistent pruning wrapper. In Context the master-only top-level ledger and the nested `master.master_wake` share one projection value; an ordinary peer keeps a null top-level field. |

The affected graphs are `collab-identity-anchor`, `collab-context`,
`collab-board-caller-read`, `collab-binding-lifecycle`, `collab-name-ownership`,
`collab-stale-identity-maintenance`, `collab-peer-lifecycle`, `collab-rex-adaptor`,
`collab-notification-consumption`, `collab-startup-recovery` and
`collab-work-role-registration` under
`docs/dagpipe/`.

## Current work and messages

| Behavior | Current gates | Required boundary |
|---|---|---|
| Same-name session continuation and explicit task reassignment | AppServer integration `name_runtime_cli`, `name_adjudication`, `board_cli`; board name/reassignment units | Update the carrier for unclosed work owned by that name. Preserve task ID, lifecycle status, evidence, resources and integration/cleanup obligations. Closed task records retain their original carrier. |
| Current-name unread mailbox handoff | `server::name_mailbox_tests`; `name_mailbox_public_repeated_release_reallocation_keeps_current_carrier`; `name_mailbox_public_distinct_names_follow_address_after_release` | The current project/name address is sufficient even after vacancy. Preserve message ID, body, sender and real consumption receipts. Other names and consumed messages do not move. Unchanged carriers do not create a new wake. |
| First peer registration notice | AppServer integration `work_roles_cli::minimal_registration_*`; `name_confirmed_takeover` initial-interruption and established-principal controls | Publish eligible worker registration, default subscription and advisory message in the same current-state business transaction. A pending initial Register with no same-principal business binding or completed operation finishes its first notice once. Established-principal reconstruction stays silent. Recovery never retires another carrier claimant. No self/master/backfill notice. Repeat Context/Register/restart stays silent. A rejected native wake retains durable mail. |
| Roles, templates and work policy | AppServer integration `work_roles_cli`, `workflow_templates`; work assignment and required-review units | Role changes and template changes notify only affected counterparts. Pinned executor/reviewer and accepted-task obligations retain their existing semantics. |
| Default role responsibilities and goal-document maintenance | AppServer integration `workflow_templates::builtin_specialty_roles_project_on_registration_and_repeated_context`, `workflow_templates::explicit_worker_peer_executes_directly_without_ordinary_peer_orchestration`, `workflow_templates::fixed_and_design_specialties_drop_orchestration_and_preserve_separation`, `workflow_templates::builtin_aliases_are_canonical_and_exact_overrides_are_not_swallowed`, `workflow_templates::specialty_reset_clear_and_role_labels_keep_authority_and_side_effects`; shared workflow projection units | First and repeated Context project built-in worker, patrol, memory and architect duties from the compiled asset. A peer with no execution specialty keeps the ordinary orchestration contract, while an explicit worker specialty executes directly without the unconditional dispatch instruction and without changing identity, authority or goal level. Fixed-duty patrol and memory roles keep their independent boundaries, and an architect plus worker mix keeps planning, implementation and final review separate. Canonical aliases deduplicate defaults but do not swallow exact `role:<name>` overrides. Reset restores a built-in default and clearing removes specialty text. Work-role labels do not grant master/CEO authority. Context explains the typed goal owner's `goal_path` entry, does not execute `appsdk goal status`, and creates no task, subscription or template revision. |
| Board lifecycle and capacity | AppServer integration `board_cli`; `server::board_state_tests`; `server::board_reassign_tests`; `server::scheduler_admission_tests` | Claim/invite/respond, delivery, review, integration and cleanup use current persisted state. Closed tasks disappear from open views. Ordinary peers require Board admission for task dispatch; ordinary messages have no role-pair restriction. |
| Explicit legacy ticket admission | AppServer integration `work_roles_cli::master_admits_legacy_rework_and_restores_start_deliver`, `work_roles_cli::legacy_admission_*`; `server::board_state_tests::legacy_admission_*` | `board backfill-execute --admit-legacy` is master-only and admits only a public, described, unadmitted `working` or `rework` task with no phase history and a current matching stable assignment. Admission commits the `admitted`/revision detail change and one `Claimed` Execute phase atomically, preserving task id, owner, history, evidence, review/close obligations and subscriptions. It does not start or deliver, leaves ordinary claim/start/deliver gates unchanged, and repeats or ineligible/unauthorized calls are explicit non-mutating rejections. |
| Master-dispatched long-horizon subscription | `cross_project_stale_identity_cli::delegated_goal_*`; `sdk_goal_blackbox`; `goal_periodic_blackbox`; goal timer units | Master task sheets declare full content and cadence. Dispatch and the target-owned schedule share the candidate-state transaction, and a capacity refusal preserves the unmodified task. New periodic goals default to100, explicit counts override it, and generic notifications keep their existing defaults. A real target tmux pane receives the precise deadline before formal recv consumes its full task instructions. Current CEO can modify a foreign project's peer schedule, while ordinary peer and foreign-master authority are refused. Content edits and restart retain schedule ID, progress and expiry. Withdrawal/end preserves ordinary reception. Existing one-shot records retain their lifecycle. |
| Business block approval | AppServer integration `block_approval`; `server::board_state_tests::pending_block_request_survives_current_state_round_trip` | An owner's request remains pending with the task running until the current supervisor approves. Self/foreign/stale decisions fail. Preserve exact pending and approved records through real state.json restart; internal resource isolation does not fabricate approval. |
| Cross-project ordinary messaging | `cross_project_stale_identity_cli` | Any registered role pair, including peer/CEO, can send directly. Preserve project isolation, exact address resolution, authenticated source and correlated reply routing. |
| Delivery versus consumption | `appserver_two_tui_integration`; `desktop_bridge_public_contract`; `dsh_master_authority_cli`; notification batch units | Durable enqueue, native acceptance, actual receive receipt and reply are separate facts. Context/inbox/status do not consume. Unknown native outcomes do not trigger blind resubmission or transport switching. |
| Subscription and patrol persistence | AppServer integration `patrol_runtime`, `patrol_subscriptions`, `project_patrol_subscribe`, `ceo_patrol_runtime`; `goal_periodic_blackbox`; timer units | Preserve the current subscription and finite schedule. Explicit periodic declaration controls repetition; ordinary Context invents no schedule. Cancelled patrols do not remove direct-message reception. |
| Master dispatch-gap reminder | AppServer integration `dispatch_gap_reminder`; `server::keepalive::tests::dispatch_gap_*` | A pending public task owned or created by the current master and a direct peer that is online, idle and eligible for that task form one gap. A sorted master+task+peer key stays silent for 600000ms after durable delivery; a changed key notifies immediately; resolution clears the key. Unknown, offline, waiting_user, busy and managed-child peers are not capacity. The reminder never dispatches or changes task state. Auto-continuation's queued-peer exclusion belongs to the same eligibility owner when that queue lands on main. |

`collab-send-message`, `collab-board-assignment-lifecycle`,
`collab-dsh-channel`, `collab-dsh-delivery` and the notification-consumption
graph bind these owners. `collab-dispatch-gap-reminder-migration` binds the
master dispatch-gap reminder. DSH first registration, delivery acceptance,
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
