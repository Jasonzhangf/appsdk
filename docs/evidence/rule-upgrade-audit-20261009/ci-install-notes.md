# ci-install worker notes

- Task ID: `rule-upgrade-audit-20261009`
- Feature: `aabed6a`
- Worker: `ci-install`
- Baseline: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- Workspace: `/Users/fanzhang/Documents/github/appsdk`
- Scope: `.github/workflows/verify.yml`, `scripts/install-global-appsdk.sh`, `scripts/tests/test-install-global-appsdk.sh`, `README.md`
- Prohibited: commit, merge, push, install, restart, installed Skill edits, global rule edits, shared config edits, product repo init

## Nodes

### 2026-10-09 start

- Status: started.
- Read: real-time global rules, `coding-principals/SKILL.md`, `codex-orchestrator/references/worker-contract.md`, task requirements/note/plan, rules report, core report.
- Baseline checks: worktree clean; HEAD is the assigned baseline.
- Finding: current workflow runs full Rust and DAGPipe tests on every push/PR and always builds release. Installer test expects stale `0.1.0010`; installer test is not wired into CI.
- Next: implement component selection, release-only full gate, installer version source, and README release boundary.

### 2026-10-09 implementation

- Status: implementation complete; static self-check complete.
- Changed:
  - `.github/workflows/verify.yml`: added changed-path component selection and independent `app-sdk-targeted`, `app-sdk-full`, `dagpipe`, `collab`, `installer`, `docs`, and `release` jobs.
  - `scripts/install-global-appsdk.sh`: release binary version must equal `rust/release-version`; existing format check remains.
  - `scripts/tests/test-install-global-appsdk.sh`: expected version now derives from `rust/release-version`; `0.1.0010` hardcode removed.
  - `README.md`: documented daily component selection, release-only full gate, minimal AppSDK release boundary, and source-tag installer path.
- Static checks:
  - `git diff --check`: PASS.
  - `bash -n scripts/install-global-appsdk.sh scripts/tests/test-install-global-appsdk.sh`: PASS.
  - Ruby YAML parse of `.github/workflows/verify.yml`: PASS.
  - workflow output-reference and dependency checks: PASS.
  - selector logic exercised with real commit ranges: branch push produced component-only outputs; tag push produced release outputs.
  - static test-name checks: guidance 15, memory 11, DAGPipe CLI 56 matches.
- Unverified by this worker: cargo tests/builds, installer test execution, real installation, GitHub Actions run, release publication.
- Next: controller runs targeted and release batches in the combined tree.
