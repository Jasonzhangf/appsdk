# DSH gateway live bidirectional round-trip verification evidence

Base: `0472fea9` (appsdk origin/main at evidence capture)
Gateway: dsh-gateway `9b5139a` (collab-notify-subscribe branch, merged to main on 2026-10-23)
Environment: macOS arm64, DSH session worker `dsh-session-4c7e81d2-686a-40a1-bbc0-d2e7d11408c6`
(codex master `codex-session-30316131326166632d623261352d373535302d386335642d363365633638396466393138`, "Jenny")
Route scope: `appserver-cli` / `/Users/fanzhang/Documents/github/appsdk`.

This evidence is a live capture of the real collab transport between a DSH
runtime and a codex master, entered through the public collab send/subscribe
surface. All facts come from the collab journal, the gateway durable queue,
and the gateway daemon log; none are simulated.

## 1. Subscription surface is usable

- `collab_notify_status` read the default `direct-message` lease armed:
  `sub-default-direct-message-dsh-session-4c7e81d2-…`.
- A controlled deadline subscription was created: `sub-m1792760501414-46`,
  due 2026-10-23T13:03:21Z, subject `goal:idle-wake-controlled-1792761232`.
- When due, the idle session really received
  `DEADLINE_REACHED subject=goal:idle-wake-controlled-1792761232; LAST_REMINDER=true`
  and a new turn started.
- Journal chain present: `NotificationSubscribed → Sent → WakeBound →
  ReceiveCommitted` (default lease) and `NotificationSubscribed → Sent →
  WakeBound → ReceiveCommitted` (deadline lease).

Conclusion: create, read, and due-wake all have real-entry evidence.

## 2. DSH → codex master, master idle

- `collab_send` returned real id `m1792758247595-32`.
- Journal: `Sent → DeliveryMode(immediate) → WakeBound(sub-m1792680623479-154)
  → WakeAttempted → NotificationDeliveryAccepted(evidence.transport=tmux,
  text_submitted=true) → ReceiveCommitted`.
- The master was idle; the message really reached its pane and its receiving
  side confirmed it.

## 3. DSH idle, codex master → DSH (idle wake)

- Controlled experiment: sent request `m1792759245250-39` asking for a
  literal reply; the DSH session then had no active turn (idle).
- The master replied `m1792759390412-40` (subject `idle-wake-ack`,
  body `DAVID-IDLE-WAKE-ACK`, `in_reply_to=m1792759245250-39`).
- The gateway durable queue recorded `enqueue mode=steer runtimeId=rt-c71a724c…
  agentId=session-4c7e81d2… at=1792759390462` plus `ack at=1792759390504`.
- Journal: `DeliveryMode(immediate) → WakeBound(sub-default-direct-message-…) →
  WakeAttempted(1792759390426) → ReceiveCommitted(recv-661c61bb…,
  received_ms=1792759390431) → NotificationDeliveryAccepted(evidence
  message_id=collab:9d41f27f…:m1792759390412-40, queued=true, sink_mode=immediate,
  transport=dsh)`.
- Gateway daemon log: `message acknowledged {messageId:
  collab-poll:d61f1cb4…:m1792759390412-40, agentId: session-4c7e81d2…,
  duplicate:false, acked:true}`.
- The body `DAVID-IDLE-WAKE-ACK` appeared in the session and started a new turn.

This is the acceptance criterion: an idle DSH session is woken by the collab
system / codex master.

## 4. Master busy, DSH → master

- `collab_send` returned a real id; journal still recorded
  `NotificationDeliveryAccepted` (transport=tmux). Busy does not block
  delivery; the message enters the pane and the runtime queues it.

## 5. Receipt separation (enqueue ≠ adapted-accepted ≠ received ≠ consumed)

- Request `m1792759245250-39` status: `read answered=true consumed=true
  wakeAttempts=1`.
- Ack `m1792759390412-40` status: `read consumed=true wakeAttempts=1`.
- Both directions carry authoritative peer-side receipts from the receiving
  identity, not local self-report. The journal records distinct facts for
  sent, delivery-accepted, received, and read/consumed.

## Notes

- `agent_state=unknown` for the DSH worker is a reporting-format difference
  only; `endpoint_live=True` and real traffic prove reachability.
- The stale `DEADLINE_REACHED subject=goal:post-reload-wake-1792730896` replay
  earlier (restart reroute) is a separate verification deadline, not part of
  this capture.