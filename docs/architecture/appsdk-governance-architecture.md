# AppSDK Governance Architecture

AppSDK provides contracts, deterministic compilation and optional workflow
guidance. Consumers own business semantics and acceptance. Repository owners
are defined in [AGENTS.md](../../AGENTS.md); development and delivery use
[appsdk-dev](../../.agents/skills/appsdk-dev/SKILL.md) and the host contract.
This document does not define another mandatory development lifecycle.

## Canonical ownership

```text
contracts/     authoring contracts; source of truth
rust/          AppSDK CLI, governance/compiler implementation
dagpipe/       independent graph library and CLI
collab/        independent identity, mailbox and collaboration product
templates/     initial project skeletons
playground/    mutable experiment source; never runtime input
active/lib/    immutable current consumable library
protected/     frozen source, contracts, and historical versions
generated/     compiler output and indexes; read-only
```

`contracts/` is authoritative for SDK machine rules. `docs/design/` explains
intent. Generated surfaces identify their source and generator. Playground,
Active, Protected and generated partitions apply only to consumers that select
that compilation contract; they do not classify SDK source packages.
Consumer `.appsdk/` owns declared contracts; `.appsdk-control/` owns local run
state. Collab central state is separate. Projections and logs never become a
second writable control source.

## Selected consumer compilation and release flow

```text
Claim + issue
  -> scope admission
  -> Playground experiment
  -> evidence
  -> architecture review
  -> architecture review PASS
  -> mainline source merge
  -> compile Active library
  -> protected source/contracts
  -> applicable release verification
  -> closeout
```

Ordinary development, planning, review and delivery do not create SHA/checksum
manifests, frozen snapshots or identity wrappers. The flow above applies only
when the consumer explicitly selects the corresponding compilation/release
capabilities. Historical records remain immutable. Existing program errors
must be reported and fixed at their owner, not bypassed with fabricated records.

## Invariants for that consumer contract

1. One feature and one resource have one formal owner.
2. Playground is mutable experiment source and cannot be imported by runtime or release builds.
3. Active is immutable consumable library, not source.
4. Promotion is the only Playground-to-mainline-to-Active path.
5. Protected source/contracts and old Active versions are immutable history.
6. Generated contains compiler output and indexes only.
7. Published semantic changes use the selected consumer's versioning contract.
8. Retired code cannot be a fallback, shadow writer, or hidden runtime dependency.
9. Logical path checks must be paired with host physical isolation.
10. Review and verification inspect the actual mainline diff and generated Active library.
11. Compilation and release claims require the actual applicable artifact and behavior evidence.
12. A passing review or compile does not prove integration, installation or live delivery.

Guidance, Memory and unenabled Collab are auxiliary. A failure blocks only work
that depends on that capability. Default feedback gives the result, necessary
references, current responsibility and actionable errors; full diagnostics are
read on demand. This does not delete durable history or message bodies.

## Visibility boundary

Protected + Git provide traceability, change detection, and recovery. They do not prevent a generic agent sharing the repository shell from reading Protected source. True read prevention requires a separate worktree/container or host mount policy.

## Consumer adapter boundary

Consumers provide their resource, verification and runtime adapters. AppSDK
must not absorb consumer protocols, providers or business pipeline semantics.
