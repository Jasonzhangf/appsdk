# Verification

Use this for changes to Collab behavior, its distributed Skill or installation.
It is not a preflight for ordinary registration, messaging or task updates.
The source project's `appsdk-dev` owns risk-based test selection; the host
workflow owns independent milestone review, Git delivery and resource closure.
This reference supplies only Collab-specific evidence boundaries.

## Select the affected evidence

| Changed behavior | Necessary evidence |
|---|---|
| Skill or design text | Semantics, real references and realistic task walkthrough; embedded-resource check when distribution content changes |
| Default output / generated prompt | Raw public CLI/MCP output for the affected consumer, necessary information retained and redundant content absent; errors and relevant overrides preserved |
| Identity / name recovery | Affected registration/restoration and owner/scope preservation cases; normal use does not add a communication round trip |
| Message delivery / consumption | Isolated real sender/recipient entrypoints; durable acceptance, selected transport outcome and target consumption checked separately |
| Subscription / wake | Affected owner, event, schedule, expiry and attempt semantics; no duplicate delivery or unintended lease loss |
| Task / review / integration | Affected typed transition, specified reviewer, pending merge and cleanup semantics; read-only delivery remains no_merge_required |
| Install / activation | Official installed entry and affected consumer; loaded runtime verified separately after user-managed activation |

The source verification map and manifest are coverage indexes, not instructions
to run every test for every edit. Keep a failure mode for each selected check.
Reuse evidence whose inputs and behavior are unchanged. A later fix invalidates
only affected conclusions; do not repeat full suites or independent reviews
for notes, status updates or local wording changes. Actual required CI gates
still apply. A filtered subset is not a full-suite PASS.

## Meaning of receipts

- `send` durable: the mailbox accepted the message. A transport error can still
  follow; retain the message ID and whether submission is known or unknown.
- Adapter accepted: the selected endpoint accepted input, not that the peer
  read, replied or completed its task. A tmux pane is not a consumption oracle.
- `recv`: the returned batch is atomically consumed; no follow-up ACK. Verify
  same-receive-ID recovery when changing consumption projection. DSH gateway
  owns consumption instead of the agent CLI.
- `task deliver` or review acceptance: the recorded stage only. Code may still
  require main integration and owned cleanup. Read-only acceptance creates no
  pending code merge. Cancellation and force-close are not successful delivery.

Use disposable projects and owned fixtures for behavior tests. Do not inject
test notices into production chats or mutate production identity/mailbox state.
Preserve the selected transport, explicit unsupported/partial/unknown outcomes,
and original failure; do not use a fallback or synthetic ACK to get PASS.
Test cleanup obeys the project tmux safety rules and never kills a server.

## Build, install and report

For an applicable official Collab release build use `scripts/build-collab.sh`;
the installer is `scripts/install-global-collab.sh`. Do not manually bump the
host build counter, copy binaries, or create a second installed entry. Ordinary
prompt/document edits do not require release builds, migration replay or live activation.

When installing, confirm the intended canonical command and installed version,
then check the affected embedded Skill/resource or public consumer. A stale
PATH is corrected at that boundary. Do not add byte-for-byte candidate packets,
SHA/checksum manifests or frozen evidence to ordinary work.

The global daemon is user-managed. Installation does not prove activation and
does not authorize an agent restart. Report source, tests, installed content,
loaded-runtime behavior, remote integration and cleanup only to the extent
relevant to this delivery; missing applicable evidence remains unverified.

Return the usable artifact, acceptance result, key evidence reference and any
remaining issue/owner. Keep raw logs available by reference; do not copy them
into each receipt or require another query merely to repeat the same result.
