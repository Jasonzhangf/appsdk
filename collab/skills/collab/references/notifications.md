# Notifications

Read this only for event subscriptions or notification delivery diagnosis.
Ordinary peer notification is always:

```sh
collab sendmessage --to <peer> --subject <short-topic> "<original message>"
```

## Subscription commands

```sh
collab notify methods
collab notify subscribe --event direct-message --ttl-seconds <bounded>
collab notify subscribe --event resource-released --subject <resource-id> \
  --ttl-seconds <bounded>
collab notify subscribe --event deadline --subject <timer-id> \
  --at-ms <epoch-ms> --ttl-seconds <bounded>
collab notify subscribe --event deadline --subject <timer-id> \
  --every-ms <interval-ms> --repeat-count <1..=100> --ttl-seconds <bounded>
collab notify status
collab notify unsubscribe <subscription-id>
```

## Patrol duties and finite schedules

A registration or dispatch reminder is not a schedule. It requires the role to
reconcile its existing scope on the next authorized activation. Do not create
empty board tasks, a fixed polling loop, or duplicate subscriptions during
context restoration. Update tasks and subscriptions when scope changes, reuse
an active valid subscription, and retire only the caller's obsolete
task-scoped subscriptions by ID.

- Master patrol covers open buglist P0/P1 work, peer progress evidence, owned
  worktree/runtime cleanup, canonical issue intake, and currently supported
  monitored events. Progress and completion require durable board, task,
  delivery, cleanup, or issue evidence; inactivity, heartbeat, and ACK prove
  neither. Close each P0/P1 with evidence or park it with owner, cause, and
  unblock condition.
- An ordinary peer owns patrol only for its accepted goal and resources. It may
  use a finite owner-scoped generic `deadline` subscription for that patrol
  when a concrete later check is needed. The direct-message lease is separate.
- Review acceptance wakes the current master only for the applicable code
  delivery merge obligation. Explicit read-only delivery (`--no-candidate`)
  reaches `no_merge_required`, creates no pending merge, and must not create a
  merge wake.
- Legacy managed records create no new patrol subscription, deadline schedule,
  dispatch, or execution authority.
- Dirty, active, shared, or unknown resources are PROTECTED_HOLD. Preserve and
  report them; never remove another owner's resource.
- A consumed one-shot wake for a goal or deadline does not renew itself. When the
  patrol must continue, explicitly re-arm it before the cycle ends. A read,
  ACK, summary, or dispatch receipt is not completion.
  Rearming is a safety net, not permission to stop: immediately continue ready
  authorized work. A future deadline or absent wake is not a task dependency.
  Apply the main Skill's Registered patrol duties for evidence-based blocker
  requests, master verification, and continued peer delivery ownership.

Run `collab notify methods` against the current daemon before selecting an
event or schedule. The current source defines `direct-message`,
`resource-released`, `deadline`, and `master-idle`; the loaded runtime's
`methods` response is authoritative. An absent event requires implementation
in its owning source, not a fake event, synthetic subject, or polling loop.
Up to three active subscriptions exist per agent.

- `direct-message` requires no subject and is reusable. `resource-released` and
  `deadline` require a non-empty exact subject. Master-idle and goal deadlines
  require the current registered master. Legacy managed records cannot create
  independent deadline schedules.
- A deadline uses repeated `--at-ms` values or `--every-ms`; the modes are
  mutually exclusive. Absolute times must be in the future and before TTL
  expiry. `--every-ms` must be positive and fire before expiry.
  `--repeat-count` is 1 through 100. TTL is 1 second through seven days.
- The command examples show syntax only, not a default patrol cadence. Do not
  invent a patrol interval or wake schedule. A finite subscription is created
  only for a concrete supported event and expires or consumes without
  automatic rearm.

- AppSDK project initialization creates/refreshes the seven-day reusable default
  `direct-message` lease through the daemon identity context. Re-registering a peer
  follows its currently selected transport binding and replaces a persisted
  default lease whose transport or target belongs to a retired route. An explicit owner
  unsubscribe of that lease stays cancelled; later `register` / `context` /
  `ack` must not silently re-arm it. Only an explicit unsubscribe stops the
  default lease. Finishing or closing the last task never cancels it, so a
  registered peer stays wakeable across task lifecycles. Task-scoped cleanup
  must not cancel or replace that default lease, so `mailbox-only` cannot
  appear silently. There is no `collab notify close` command; cancel a
  specific owner-scoped lease with
  `collab notify unsubscribe <subscription-id>`. After an explicit
  unsubscribe, `collab notify subscribe --event direct-message` re-arms it for
  the next collaboration.
- Direct-message leases are owner-scoped and reusable until expiry. Resource
  subscriptions are exact-subject and one-shot. Generic deadline subscriptions
  consume their explicit finite occurrence count; `goal:` deadlines remain
  one-shot and owned by the subscribing master or ordinary peer. Only
  `master-idle` requires current-master authority. A peer's goal deadline must
  wake that peer and must not alter the master's wake accumulator.
- Success consumes only a one-shot event subscription. Expiry or unsubscribe
  ends any subscription; one attempted batch exhausts only its messages on a
  reusable direct-message lease.
- Before every attempt, the daemon revalidates owner, event, subject, TTL,
  selected transport ownership and liveness, worker registration match, and
  presence. An unreachable, unowned, or mismatched endpoint transitions to its
  explicit unavailable state. `absent` and `unknown` produce zero transport input.
- Timer ticks, restart, replay, re-registration, or delivery mode cannot reset
  the one-attempt lifetime cap. An AppServer binding uses the selected native
  RPC (`turn/start`, `turn/steer`, or `thread/queue/add`); a tmux-only binding
  uses its selected pane adapter. The daemon never falls back across bindings.
  Transport acceptance does not prove execution, read, reply, task progress,
  or lifecycle.
- Unacknowledged notification throttling (Backpressure): To prevent notification
  storms and terminal pollution, push knocks pause when unacknowledged notifications
  reach `max_unacked` (default 3, range 1-5). Run `collab ack <id>` or `collab ack --all`
  to resume push delivery. Messages remain safely buffered in the durable mailbox.
- The bound subscription event policy supplies the automatic retry window: the
  default `direct-message` batch window is 120 seconds, while an event configured
  as `immediate` has a zero retry delay. At dispatch, include eligible unsent
  messages for the recipient, capped at 3 previews per batch knock. Excess
  messages remain in the inbox with `[+N more pending in inbox]`. When unacked
  notifications reach the throttle cap, the batch preview appends an
  `[ACK REQUIRED: ...]` notice. Combine previews into the selected transport
  operation and reserve attempts before sending. Failed, absent, unknown, or uncertain
  ordinary delivery remains pending; automatic eligibility still respects the
  bound event window and the lifetime attempt cap. Explicit notification
  delivery is never an automatic timer candidate; a later explicit operation
  must be used when the failure proves submission did not occur. An ambiguous
  post-submit transport outcome is not automatically retried for that
  message: the mailbox entry remains durable, and the recipient should run
  `collab recv`; send a new message if another wake is needed. The send error
  includes the durable message ID and states this recovery path.
- P0 urgency is explicit: only a typed goal/deadline interrupt is marked P0. A blocked
  task, wait-timeout, or scheduling blocker is P1 operational work and must not be
  reclassified as a P0 interrupt by subject text alone.
- One safe preview contains notification ID, abbreviated subject, and one-line
  original body. Control characters are escaped. Explicit `collab sendmessage`
  and daemon-generated wakeups use the already selected binding; transport
  acceptance does not create a receive receipt. Only `collab recv` consumption
  commits the durable receipt visible to the sender.
- Full subject/body remains in the mailbox without a matching subscription.
  This outcome is not a sender-selected `mailbox-only` mode.
- A failed/lost/delayed/duplicate wake never rolls back mailbox truth or counts
  as lifecycle evidence.
- The daemon never infers continuation from task state, thread title, heartbeat,
  progress, ACK, or elapsed time, and must not create a `CONTINUE_TASK` message.
  The task-liveness rule is an Agent/skill obligation: when a supported
  timer/wake or direct wake arrives, the owner inspects durable task state and
  continues or escalates it. It is not a second queue or a synthetic periodic
  task. A worker does not poll on a fixed internal schedule.

## Worker inspection and acknowledgment

```sh
collab ack <notification-id>
collab ack --all
collab worker status [worker-id]
collab who
```

- `collab ack <id>` acknowledges a single notification.
- `collab ack --all` acknowledges all unread or pending delivered notifications in one step.
- `collab recv` reads and consumes the returned message batch atomically; a
  successful receive writes `Delivered` and `Acked` together, so a follow-up
  ACK is not required. `collab msg`, `collab inbox`, and `collab context` are
  read-only and do not consume messages. Keep `ack` for legacy clients or
  explicit recovery of already-delivered messages.
- `collab worker status [worker-id]` inspects the selected transport's address
  facts and worker health, including `endpoint_live`, `identity_valid`, `agent_state`,
  `unacked_notifications`, `notifications_paused`, `suspected_offline`, and
  `active_task`.

After a preview, compare its ID/subject, urgency, current task, and interruption
cost. Read durable details only when appropriate:

```sh
collab msg <notification-id>
collab inbox
collab context
```
