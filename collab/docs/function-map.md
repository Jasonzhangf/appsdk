# Collab current function map

| Behavior | Unique owner and entry | Required boundary |
|---|---|---|
| Register/recover/rebind | `identity_context/mapped_context.rs`, `BindingControlStore` | Identity selection reads only the current map. Preserve actual host facts, exact credentials and original errors. |
| TUI naming | `identity_context/anchor_rename.rs`, `tm_layout.rs` | Fixed shared position; rename/readback before publication; rollback unpublished failures only. |
| Master authority | Mapped role change and existing grant owner | Explicit authorization; no authority inferred from name, position, activity or stale grant. |
| Current persistence | `current_state::{load,save}`, `commit_locked_checked` | Persist direct current State, with no event history or replay. Preserve data on failure and report unknown durability truthfully. |
| Board assignment/lifecycle | Board/task handlers and required-review owner | Current admission and revision checks, stable-name continuation, explicit task handoff and lifecycle obligations. |
| Business block approval | `task_block_apply_locked`, `task_block_decide_locked` | Owner requests pending; current supervisor decides; self/foreign/stale decisions cannot change task state. |
| Task conflict/wait | `task_conflicts`, `handle_task_wait`, timers | Bounded acyclic wait on the exact conflicting resource. Master repair for an ownerless task preserves its persisted owner. Release matching and notification use the durable `WaitSpec.waiter`; the existing persisted-owner fallback remains only when a waiting task has no wait record. |
| Named mailbox | `name_mailbox::handoff_events`, receive owner | Move unread messages by current scoped name, including a vacant predecessor. Preserve real consumed receipts and payloads. |
| Native notification | `attempt_notification_with`, notification contract/adapters | Durable current message before external send; acceptance, actual receive and reply remain distinct. |
| Manual communication | `identity_context/communication.rs::mapped_communication` | Ordinary authenticated self-Send, exact current binding, real receive and correlated self-reply; no fabricated consumption or blind retry. |
| Context/query/status | Context projection and host adapters | Current identity and scoped work projection; query stays read-only, liveness remains an observation. |
| Scoped route | `ProjectRuntimeManager`, route-resolution entry | Exact current native/tmux binding, scoped storage and explicit absence/ambiguity. Route indexes do not select an identity. |
| Project reset/startup | Project reset owner, current runtime setup | Finish committed reset before restoration, retain foreign data, surface current-file errors; reset admission is independent of removed migration APIs. |
| User activation/shutdown | Formal service entry and authenticated Shutdown owner | User-managed production lifecycle, complete response before stop, cancellation of blocked writes and owned PID/socket cleanup. |

Public gates and original evidence belong in [verification-map.md](verification-map.md)
and the task run. This map does not assert installation or live acceptance.
