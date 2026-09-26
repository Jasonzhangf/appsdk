# Merge-pending DAG 测试计划

任务：`review --accept` 登记 daemon-owned pending merge，master 必须 merge 并
`task integrated` 后才能 close；每次 idle 提醒与 `appsdk longhorizon show` 都能看到。

范围：`collab/src/server/{state,mod,keepalive,timers,mailbox}.rs`、
`rust/src/main.rs`、skill 与 DAG 文档。

## Checklist

- [x] T1 单元：`review --accept` 登记 pending_merges 并生成 master `merge-pending:` 通知
- [x] T2 单元：pending 期间 `task close` 返回 `TASK_MERGE_PENDING`
- [x] T3 单元：`task integrated` 解除 pending 并可 close
- [x] T4 单元：accepted→rework 解除 pending
- [x] T5 单元：journal 快照/回放保留 pending_merges（daemon 重启后不丢）
- [x] T6 单元：master idle 批次 body 含 `pending_merges`
- [x] T7 CLI：`appsdk longhorizon show` 渲染 `待合并` 区块 + `--json.pending_merges`
- [x] T8 回归：collab bin 全量、collab-mcp 全量
- [x] T9 隔离 live：两个 tmux session 走 register→deliver→review accept→integrated→close
- [x] T10 隔离 live：daemon 重启后 pending merge 仍可见，close 仍被拒
- [x] T11 安装验证：候选 binary 安装、daemon 重启、真实 `collab status --all` 投影
- [ ] T12 独立 review（Codex + AGY）PASS

## 命令与期望

| 检查 | 命令 | 期望 |
| --- | --- | --- |
| T1-T4 | `cargo test --manifest-path collab/Cargo.toml --bin collab peer_tests::` | 0 failed |
| T5 | `cargo test --manifest-path collab/Cargo.toml --bin collab state::tests::` | 0 failed |
| T6 | `cargo test --manifest-path collab/Cargo.toml --bin collab timers::` | 0 failed |
| T7 | `cargo test --manifest-path rust/Cargo.toml --test cli_smoke longhorizon` | 0 failed |
| T8 | `cargo test --manifest-path collab/Cargo.toml` | 仅环境竞态 socket/tmux 用例可单跑复现 |
| T9-T11 | 见"隔离 live 步骤" | 每步保留 command + 输出 |

## 隔离 live 步骤（T9/T10）

1. 在 `/tmp` 建隔离 git 项目，`collab` 用隔离 `HOME`/socket，起两个 tmux session。
2. session A 注册 owner，session B 注册 master 并 promote（隔离项目内授权）。
3. A 建任务→deliver→B `review --accept`；断言 `collab status --all.pending_merges`
   含 task id，B 的 `collab context.operations` 含 `merge_pending`。
4. A 尝试 `task close`；断言 `TASK_MERGE_PENDING`。
5. B merge 到 main 并 `task integrated`；断言 pending 消失、close 成功。
6. 重启隔离 daemon，重复步骤 3 的断言（T10）。

## 失败终点

| 失败 | 语义 | 处理 |
| --- | --- | --- |
| 全量测试仅 socket/tmux 竞态失败 | 环境既有 flaky | 单跑复现通过即记录，不掩盖 |
| live 无法建隔离 daemon | 环境限制 | 报告 UNVERIFIED，不用源码测试冒充 |
