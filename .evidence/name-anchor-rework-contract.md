# 任务合同：身份名字锚定实现收口（阶段 B，基于最新 main）

派发者：Jenny（appsdk master）
执行角色：一次性执行 worker（`codex exec --profile gcm`）
工作树：/Volumes/Intel/playground/appsdk/collab-session-name-anchor-20261103
分支：codex/collab-session-name-anchor-20261103
基线：origin/main 6f641805（包含已集成候选 98157b5b）

## 目标

把既有未提交的名字锚定实现收口为可提交、可审查、可集成的候选：

1. TUI 首次注册后把 live tmux session 改名为角色注册名并读回确认；改名失败不提交身份、
   保留原名。已有固定 session 名恢复时按该名续接，不改名、不重分配。
2. Codex Desktop 与 DSH 不做 tmux 改名，但仍以各自 session 名字为身份/投递/任务锚点。
3. 名字是唯一锚点；旧 journal 一次性迁移后作废；tmux $N、pane、socket、PID 仅定位。
4. `collab/src/adapters/tmux.rs` 的 `observe_session_name`、`rename_session`、
   `session_name_if_live`、`validate_session_name` 与 context/master promote 的
   rename/rollback/swap 路径成立。

## 范围（allowed）

- 仅在该工作树内编辑 `collab/`、`docs/`、`.appsdk/` 与相关测试文件。
- 先与 `origin/main` 集成：允许在分支上提交当前 WIP，再 `git merge origin/main` 解决冲突，
  或 `git rebase origin/main`；不得修改或丢弃任务范围外的 dirty 文件。
- 运行 cargo 构建与测试（`CARGO_HOME=/tmp/cargo-home`，`-j 1`）。
- 在本分支上创建本地提交。

## 禁止（forbidden）

- 不得安装到 `~/.cargo/bin`，不得改动或重启 live daemon，不得跑任何真实 daemon 的
  `collab context` 写操作。
- 不得 `tmux kill-server`、不得 `kill-session -a`、不得 `pkill`/`killall`、不得批量终止；
  测试清理只用测试自有 socket 的具名 session。
- 不得新建或删除 git worktree；不得 push、不得合并 main。
- 不得改动主工作树 `/Users/fanzhang/Documents/github/appsdk`。
- 不得为了变绿跳过、删除或弱化断言。

## 已确认事实

- 既有未提交改动 19 文件、约 746 行；工作树基于 6fd56653，落后 origin/main 19 个提交。
- 阶段 A 编译通过；`public_context_fixes_the_tmux_session_name_and_restores_by_it`
  单独通过；全量单线程 `context_operation_public_contract` 52 passed / 28 failed，
  首因是 `c12_sigint_before_admission_returns_local_cancelled_without_daemon_operation`
  barrier 超时，其余为 PoisonError 级联。

## 验收（done-iff）

1. 先在干净基线（origin/main 6f641805，无本次改动）单线程跑
   `cargo test --manifest-path collab/Cargo.toml -j 1 --test context_operation_public_contract -- --test-threads=1`，
   记录 passed/failed，判定失败是否为本次改动引入。
2. 修复真实失败（实现缺陷或测试时序/fixture 缺陷都属本范围），使该套件单线程全绿；
   保留原始失败证据与修复理由。不因候选基线本来失败而跳过。
3. 额外跑：`cargo test -p collab name_resolution --tests`、`cargo test -p collab
   session_name_is_observed_and_rename_is_idempotent`、
   `rename_session_rejects_malformed_fixed_name`、
   `failed_rename_is_explicit_and_keeps_the_previous_name`。
4. 在分支上创建本地提交，输出 commit sha、完整 diff stat、每条命令的原始结果行。

## 交付

回执含：基线对比结果、修复点与理由、各测试原始结果行、commit sha、`git status` 是否干净。
若无法修复，保留原错、失败样本路径与已尝试方案，标 BLOCKED，不做降级或绕过。
