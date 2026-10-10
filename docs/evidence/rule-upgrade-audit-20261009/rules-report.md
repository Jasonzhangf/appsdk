Audit complete. Read-only, baseline `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` unchanged, worktree clean.

**Deliverables** (worker run dir, nothing else written):
- [report.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/rules/report.md) — full report
- [notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/rules/notes.md) — process log + observation batch

**What I did:** inventoried all in-scope Skills/references/machine contract, `contracts` + current maps + transitions, `templates/minimal`, install scripts + `.github`, `README`, and the current architecture/design/migration docs; traced only the consumer edges needed (`main.rs`, `governance.rs`, `init.rs`, `migration.rs`, `intake.rs`, `review_context.rs`); and diffed source Skills against the installed `~/.agents/skills` counterparts (byte-identical). No tests/build/install/restart/agent dispatch.

**Findings (9 facts + 3 hypotheses), highest-signal:**
- **F1/F2 install+CI gap:** `scripts/tests/test-install-global-appsdk.sh:90` pins `appsdk 0.1.0010` while source is `0.1.0012`, and the script is referenced by no CI/README/script. CI (`verify.yml`, the only workflow) runs no `scripts/*.sh` and no `collab` tests — the install transaction has no passing automated check.
- **F3/F4/F5 transition drift:** `zone-transition-matrix.md:16` names the hyphen path as the machine source, but the real data is the dot file (the hyphen root file is a JSON Schema). The template ships a stale duplicate transition contract (missing whitebox/blackbox/pre-review gates) that nothing reads, and `zone-transition.schema.json` is an orphan.
- **F6/F7 install-surface docs:** the retained inventory says to keep a `~/.codex/skills` link, but the installer only writes `~/.agents/skills` and the link is absent; `appsdk-global-registry.md:28-30` calls `~/.local/share/appsdk` bundles installer-managed, but no script writes there.
- **F8/F9 duplication:** a new project gets contracts in two roots (root `contracts/**` and `.appsdk/contracts/**`); the manifest omits `module-registry`; and the 641-line governance SKILL.md is installed twice (`rules` and `skills` zones).

The report also reconstructs the source→release DAG, lists a minimal delete/merge/narrow plan (no new ledger/scheduler/schema), and separates static findings from the unverified runtime/`collab`/`dagpipe` areas. Key caveat: `appsdk new`/`init` was not executed here, so layout claims come from code reading plus the CI `release` smoke assertions, not a local run.