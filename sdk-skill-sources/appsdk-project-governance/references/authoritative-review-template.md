# Authoritative Requirement Review Template

This is the AppSDK-owned source template for a review packet. It defines the
fixed SDK review duties and the project facts that an executing agent must
load or provide. It is not a requirement store, an authorization service, a
runtime command, or a second review schema.

The SDK bundle distributes this source as an installed resource. The user's
explicit conversation instruction authorizes the specified change.

## Authority and roles

| Artifact | Authority |
| --- | --- |
| Review procedure and fixed reviewer duties | AppSDK review governance owner, using this template |
| Effective project requirement, original text, version, and change history | Project requirement owner and its declared authoritative source |
| Candidate, scope, design, implementation, and author evidence | Task author |
| Requirement change authorization | The user's explicit instruction in the conversation, preserved with its source |
| Review findings | Independent reviewer |
| Final admission decision | Existing AppSDK review controller or gate, using the reviewer's evidence |

An executing agent may assemble project facts and evidence references. It must
not create a user instruction or edit this template for a task. It may transcribe
the actual user instruction and source into the requirement change record. A reviewer may report that
authorization is absent, invalid, or unreadable. A reviewer must not approve a
requirement change for the user.

## Fixed SDK obligations

The executing agent must satisfy these duties before dispatch:

1. Read the effective requirement from the project-declared authoritative
   source and record its exact identity and version.
2. Preserve the user's original requirement text and acceptance criteria. A
   plan, design, code summary, test result, or author message cannot replace
   them.
3. Load the prior reviewed version when a change is claimed. Record the exact
   user change instruction and source for every changed, replaced, revoked, or
   removed requirement item.
4. Bind the packet to the exact review stage, candidate, base, scope, allowed
   paths, forbidden paths, and applicable evidence.
5. Map every applicable requirement item to its design or implementation
   reference and to the available stage evidence.
6. Leave missing, conflicting, or unreadable required sources visible. Do not
   fill the gap with an agent-authored interpretation.
7. Reuse the backend-supplied JSON output contract and the existing AppSDK
   EvidenceRecord and ReviewRecord contracts. Do not add a local review schema
   or a second evidence store.
8. For a design review, provide the design, DAG, owner/path mapping, capability
   evidence, and planned acceptance path. Do not require tests for code that is
   not implemented yet.
9. For an architecture review, provide the exact candidate and the author's
   applicable development, E2E, black-box, runtime, and side-effect evidence.
   A design statement cannot replace executed behavior evidence.
10. State that the reviewer is independent. The author must not write the
    reviewer's finding or admission decision.

The reviewer must independently read the declared sources, compare each
applicable item with the current version and any change authorization, and
record the requirement version plus the item-to-design/implementation/evidence
mapping. The reviewer cannot rely on the packet's transcription as proof.

For a project with an established requirement ledger, load all effective items
and their history from `.appsdk/requirements.json` through `appsdk requirements
show` and `history`. The task goal's `requirements_version` is a reference to
the ledger version. The SDK review context supplies the complete ledger under
`long_term_requirements`; an author must not replace it with selected items.
Compare the recorded user instruction with its conversation source and the
previous requirement version. Conversation authorization is sufficient; do not
require biometric authentication, signatures, or an external identity service.

## Agent-filled review packet

Copy the packet below into the review dispatch. Replace every placeholder with
observed project facts. Keep an unavailable field as `not available` with its
reason and evidence. Do not invent a command, path, version, or approval.

```markdown
# Authoritative Requirement Review Packet

## 1. Review identity

- Stage: design | architecture
- AppSDK architecture context_id (exact public review-context output):
- Architecture observation acknowledgement: requirements_review with that context_id and checked: true. With a fixed backend JSON schema, the reviewer records the exact context_id and explicit check in the existing evidence fields; the lifecycle adapter may map that received acknowledgement into the observation. It must not invent a check or add fields to the backend schema. The executing agent must not fill the reviewer's acknowledgement.
- Project:
- Task/issue:
- Requirement owner:
- Authoritative source owner:
- Unique read path or reference:
- Exact source version/revision:
- Source read result and evidence reference:

## 2. Candidate and scope

- Review mode: base | commit | uncommitted
- Candidate commit/tree:
- Base:
- Scope paths:
- Allowed paths:
- Forbidden paths:
- Applicable modules/owners:
- AppSDK candidate/evidence records:
- Existing ReviewRecord target or reference:

## 3. Authoritative requirement

- Requirement ID:
- Effective status:
- Effective version:
- User original text:
- Normative statement:
- Acceptance criteria:
- Applicable scope:
- Source and version evidence:

## 4. Applicable item map

| Item | Authority version | Requirement text or exact quote | Acceptance | Candidate/design reference | Implementation reference | Stage evidence reference | Reviewer result |
| --- | --- | --- | --- | --- | --- | --- | --- |
|  |  |  |  |  |  |  | pending |

## 5. Requirement changes

- Changed since the prior reviewed version: yes | no
- Prior version:
- Current version:
- Exact user change instruction:
- User source:
- Covered requirement items:
- Items explicitly replaced, revoked, or removed:
- No-change statement, when applicable:
- Agent interpretation, if any, and why it is not authorization:

If no change is claimed, state: `No requirement change authorization is
claimed.`

## 6. Current-stage evidence

### Design review

- Design artifact:
- Design DAG and entry/exit:
- Owner and affected path binding:
- Capability and dependency evidence:
- Planned acceptance and evidence path:
- Known blockers or unavailable sources:

### Architecture review

- Exact candidate artifact/build reference:
- Author development test evidence:
- Author E2E/public-entry evidence:
- Black-box success, failure, and side-effect evidence:
- Install/restart/runtime evidence, when applicable:
- Evidence freshness and candidate binding:
- Known blockers or unavailable sources:

## 7. Backend and AppSDK record mapping

- Backend-supplied JSON schema reference:
- `contract_version`:
- `scope.mode`, `scope.commit`, `scope.base`:
- `module_boundary_evidence[].resources` authority source/version:
- `module_boundary_evidence[].edges` requirement-to-design/implementation mapping:
- `module_boundary_evidence[].gates` requirement-to-evidence mapping:
- AppSDK EvidenceRecord references:
- AppSDK ReviewRecord reference:
- Any existing-contract gap:

Do not add fields to the supplied backend schema. If the existing AppSDK
records cannot express a required fact, report the gap to the AppSDK review
governance owner.

## 8. Reviewer instruction

Read the authoritative source directly. Verify the original text, version,
acceptance, change instruction, candidate scope, and stage evidence. Compare
each applicable item with the current requirement and the prior version. Do not
use the plan, author summary, current code, or a successful test as a
replacement for the authoritative requirement.

Record the source/version you read and the item-to-design,
item-to-implementation, and item-to-evidence correspondence in the existing
backend output fields. Return findings for missing sources, stale versions,
unauthorized changes, reduced acceptance, missing required evidence, or a
candidate/scope mismatch. Do not output a verdict. The controller decides.
```

## Blocking conditions

Return a blocking finding when any applicable condition is true:

- The authoritative source, effective version, original text, or acceptance
  criteria is missing, unreadable, stale, or internally conflicting.
- A requirement was changed, replaced, revoked, removed, or given lower
  acceptance without an exact user instruction and source.
- A requirement change is supported only by an agent-authored summary,
  confirmation field, plan, current implementation, test result, or
  recomputable hash.
- The candidate, base, scope, paths, or requirement version does not match the
  review material.
- A design review lacks the applicable design closure or capability evidence.
- An architecture review lacks the author's applicable public-entry behavior
  evidence for the exact candidate.
- An applicable requirement item has no design, implementation, or evidence
  mapping.
- The backend output is missing, invalid, uses a new schema, or cannot be tied
  to the existing AppSDK evidence and ReviewRecord.
- The packet asks the reviewer to approve a requirement change, edit the
  template, or invent a user instruction.

## Stage boundary

| Stage | Read | Do not require |
| --- | --- | --- |
| Design review | Effective requirements, source/version, change authorization, design, DAG, owner/path binding, capability evidence, planned acceptance | Implemented code, implementation tests, deployed runtime receipts |
| Architecture review | The same authority material plus exact candidate, author development/E2E evidence, public-entry black-box evidence, and applicable runtime evidence | A new requirement source, an agent-created authorization, or a second review schema |

The current stage determines the evidence boundary. Do not promote a design
statement into architecture evidence. Do not demand future implementation
evidence during design review.

## Limits

This template provides a procedure and a packet shape. It does not prove that a
requirement was authenticated, that the authoritative store is physically
tamper-proof, or that the template was distributed. Those claims require the
separate owner, contract, and live evidence. Until then, report them as
unverified and block only the affected admission path.
