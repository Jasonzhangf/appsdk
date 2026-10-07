# W3 report: AppSDK authoritative requirement review template

Status: DONE for the allowed documentation-source scope.

## Changed files

- `sdk-skill-sources/appsdk-project-governance/references/authoritative-review-template.md` (new): AppSDK-owned reviewer template and agent-filled packet.
- `sdk-skill-sources/appsdk-project-governance/references/review-delivery.md`: links the review flow to the template and defines the required dispatch material.
- `sdk-skill-sources/appsdk-project-governance/SKILL.md`: minimal discovery and review-gate entry links.

## Semantics

The template separates fixed SDK review duties from project-supplied facts. It
requires the executing agent to read the declared authoritative source, record
the exact version and original text, preserve acceptance criteria, load the
prior version when a change is claimed, and record explicit user change
authority. The independent reviewer must read the source directly, verify each
applicable item, map requirements to design or implementation and stage
evidence, and block missing sources, stale versions, unauthorized changes,
reduced acceptance, mismatches, or missing required evidence.

The template reuses the backend-supplied JSON output contract and existing
AppSDK EvidenceRecord/ReviewRecord references. It adds no schema and no review
database. It distinguishes design-review evidence from architecture-review
evidence and does not require future code tests at design review.

## Verification

- `git diff --check -- <three allowed files>`: no output.
- New untracked file checked with `git diff --no-index --check /dev/null <file>`: no whitespace diagnostics; exit 1 is the expected file-difference status.
- Relative template link exists at the Skill and `review-delivery.md` entry points.
- Manual check found no invented CLI command in the template.
- Existing review-delivery obligations remain; the tracked change is additions only.

## Limits and handoff

This worker did not package, install, distribute, or run the template. No
authentication, authorization, tamper-protection, runtime, or black-box
behavior evidence exists for the template. A later packaging owner must connect
the source to the SDK bundle and prove the consumer path. The installed
`/Users/fanzhang/.agents/skills/appsdk-project-governance/SKILL.md` currently
does not reference the new template, so the source links are not distributed
Skill links yet. This worker made no Rust, JSON, test, version, global Skill,
commit, merge, or install change.

Worker records:

- `/Volumes/Intel/playground/appsdk/.worker-runs/authoritative-review-template-20261007/w3-template/notes.md`
- `/Volumes/Intel/playground/appsdk/.worker-runs/authoritative-review-template-20261007/w3-template/report.md`
