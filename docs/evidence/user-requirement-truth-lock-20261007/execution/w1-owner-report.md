# W1-report: SDK authoritative review template handoff

Status: DONE (read-only consolidation).

Base: `c3c0c8df79e69534fe30c92db61328824473d5c0`.
Worktree: `/Volumes/Intel/playground/appsdk/authoritative-review-template-20261007`.
Scope: retained W1 observations plus the named W2/owner slices. No product, config, Skill, build, test, install, daemon, Git, or resource change.

## 1. Known: template distribution

The formal template source is:

```text
sdk-skill-sources/appsdk-project-governance/references/authoritative-review-template.md
```

The logical bundle source must be:

```text
skills/appsdk-project-governance/references/authoritative-review-template.md
```

The current `SDK_BUNDLE_RESOURCES` in `rust/src/main.rs:80` does not include this template. `contracts/sdk-bundle.manifest.json` `resources.skills` does not include it either. W3 has only created the source file and linked it from `review-delivery.md`; distribution is still pending.

Minimum wiring:

1. Add one `"skills"` entry to `SDK_BUNDLE_RESOURCES` in `rust/src/main.rs`, using the logical path above and:

```rust
include_str!("../../sdk-skill-sources/appsdk-project-governance/references/authoritative-review-template.md")
```

2. Add the same logical path to `resources.skills` in `contracts/sdk-bundle.manifest.json`.

`rust/src/main/governance.rs:185-213` requires the manifest set and embedded set to match exactly:

- manifest entry without an embedded entry fails `SDK_BUNDLE_MANIFEST_MISMATCH:<source>`;
- embedded entry without a manifest entry fails `SDK_BUNDLE_RESOURCE_SET_MISMATCH`.

Installation path is deterministic through `sdk_resource_install_relative` (`governance.rs:160-183`):

```text
.appsdk/skills/appsdk-project-governance/references/authoritative-review-template.md
```

`install_bundle_resources` (`governance.rs:243-289`) writes that file and records its source, class, path, and digest in `.appsdk/sdk-resources.json`. No separate install mapping is needed.

Compatibility note: adding the resource changes `bundle_digest`. The current manifest remains `0.1.0010`. If this is part of the same unreleased SDK candidate, no extra migration may be needed; otherwise the release owner must apply the normal version/migration policy. Do not hand-edit installed mirrors.

## 2. Known: review owners

Current unique owners:

| Concern | Owner |
| --- | --- |
| Public admission CLI | `rust/src/main/cli.rs:14-29`, `appsdk verify --review-admission [project] --module <id>` |
| Admission gate | `rust/src/main/review_gates.rs:3`, `verify_review_admission` |
| Architecture review record generation | `rust/src/main/verification.rs:3` dispatch to `rust/src/main/lifecycle_closure.rs:421`, `lifecycle_chain_architecture` |
| ReviewRecord schema | `contracts/records/review-record.schema.json` |
| Review identity generation and verification | `rust/src/main/producer.rs:785-837`, `lifecycle_chain_review_identity` and `assert_lifecycle_chain_review_identity` |
| Candidate/evidence/admission binding checks | `rust/src/main/review_gates.rs:258-399`, `assert_fix_architecture_gate` |
| Current task requirement record | `.appsdk/goal.json`, read and validated by `rust/src/main/registry.rs:663-806` |

`project_bindings` is already an allowed ReviewRecord object and is included in the review identity (`producer.rs:785-804`). Existing test `lifecycle_chain_architecture_binds_project_bindings_to_review` proves that changing it changes `review_id`.

## 3. Minimal requirement binding

Use the existing template and records. Do not add a database, authentication service, second requirement store, or another digest gate.

Minimal flow:

1. The template remains the SDK-owned source. Its existing sections already reserve requirement ID, effective version, original text, acceptance criteria, source/version evidence, prior version, and change instruction.
2. A public assembly entry, currently absent, loads the current requirement through the existing goal-record owner (`registry.rs`), not from the global Skill and not from agent-authored prose. It computes the requirement version once with the existing digest helper used by that owner.
3. The assembly output contains the template version/reference, requirement ID/version/source digest, original text and acceptance criteria, candidate/base/scope, and evidence references. This is a derived review packet, not a new requirement truth source.
4. The reviewer returns the existing backend JSON contract. Human-readable source and mapping stay in existing `module_boundary_evidence.resources`, `edges`, and `gates`; no new review schema.
5. `produce-lifecycle-chain --phase architecture` copies only the requirement reference/version/source digest into `project_bindings.authoritative_requirement`. Do not copy the full requirement text into ReviewRecord. Because `project_bindings` already enters `review_id`, the record is bound to that exact requirement version.
6. `verify_review_admission` recomputes the current requirement version through the same owner and compares it with `project_bindings.authoritative_requirement`. Missing, unreadable, or stale binding blocks admission. The assembly command and admission must call one shared helper; do not add a second independent hash policy.

Proposed public interface, still to implement:

```text
appsdk review assemble [project] --module <id> --phase design|architecture --input <json>
```

The exact command name is not frozen by current code. The DAG operator is `appsdk.requirements.assemble_review_packet@1`; the graph explicitly says binding is not implemented. Keep the command read-only and print the derived packet; do not create a new persistent store.

Compatibility risk:

- Existing architecture callers do not provide `project_bindings.authoritative_requirement`. Making it mandatory for every current caller is a breaking contract change.
- Minimal compatible path: the new assembly entry always requires the source; the existing record producer validates the binding when present. This preserves legacy records but does not make the requirement lock universal.
- Universal enforcement requires updating all architecture fixtures and callers, and should be treated as a versioned behavior change.
- `.appsdk/goal.json` is the current task goal record. Its `confirmed_by`/`confirmed_at` are not user authentication. Long-term requirement history, authorization, and physical tamper resistance remain `UNVERIFIED` and outside this increment.

## 4. Public black-box entry points

Reusable existing tests:

| Test | Fixture/entry |
| --- | --- |
| `rust/tests/cli_smoke/part_07.rs::install_bundle_resources_projects_declared_record_contract_sources` | `temp_root("project-record-contract-projection")`; `appsdk new`; reads `.appsdk/sdk-resources.json` |
| `rust/tests/cli_smoke/part_05.rs::lifecycle_chain_architecture_binds_project_bindings_to_review` | `prepare_lifecycle_chain_fixture` in `part_08.rs`; `produce-lifecycle-chain` |
| `rust/tests/cli_smoke/part_06.rs::lifecycle_chain_rejects_tampered_persisted_review_identity` | same lifecycle fixture; tampered `project_bindings`/`review_id` |
| `rust/tests/cli_smoke/part_12.rs::full_module_freeze_and_active_publish_require_record_graph` | `temp_root("full-lifecycle")`; `write_records`; `appsdk verify --review-admission` |

Required new black-box tests, names recommended:

1. `review_packet_installs_and_assembles_from_consumer_without_global_skill`
   - Fixture: `temp_root("review-packet-consumer")`.
   - Run `appsdk new`, read the installed template and `.appsdk/sdk-resources.json`, then run the new assembly entry.
   - Assert exact requirement ID/version/original text/acceptance criteria and installed resource path/digest.

2. `review_packet_blocks_missing_or_stale_requirement_source`
   - Fixture: same consumer fixture.
   - Remove/alter `.appsdk/goal.json` or mismatch the recorded version.
   - Assert non-zero exit, explicit source/version error, and no review record written.

3. `review_record_binds_authoritative_requirement_version`
   - Fixture: reuse `prepare_lifecycle_chain_fixture`.
   - Produce architecture input with `project_bindings.authoritative_requirement`.
   - Assert ReviewRecord contains the binding, `review_id` changes when the requirement digest changes, and `verify --review-admission` rejects a stale binding.

## 5. Gaps

- Known: bundle distribution path and install path.
- Known: admission, generation, identity, and current goal-record owners.
- Pending: template entry in `SDK_BUNDLE_RESOURCES` and manifest.
- Pending: public assembly command and its input/output contract.
- Pending: requirement binding validation in admission and black-box tests.
- Unverified: user authentication, long-term requirement history/authorization, and physical tamper protection.
- Unverified: whether parent will require the binding universally or preserve legacy callers through a versioned migration.
