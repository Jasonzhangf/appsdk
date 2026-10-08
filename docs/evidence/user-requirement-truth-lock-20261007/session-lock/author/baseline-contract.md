身份：独立GCM worker，parent当前目标编排者。不注册Collab。你不是唯一worker，保留他人改动。
目标：解除当前候选继承的source registry测试文件行数阻塞，完成 iff 仅机械拆分超限测试文件且原测试均通过，source registry不再报此文件。不得改产品运行语义或放宽门禁。
工作位置：/Volumes/Intel/playground/appsdk/requirements-session-lock，base8555a74e5e46de10136fda9539ae044d3e8bdfbc。
先读：/Users/fanzhang/.agents/AGENTS.md、/Users/fanzhang/.agents/skills/codex-orchestrator/references/worker-contract.md；collab/src/server/global_state_tests.rs；相邻已拆分测试的include模式；contracts/maps/module-registry.json的collab owner。
事实：installed appsdk verify-sdk-source-registry当前真实报 SDK_SOURCE_LINE_LIMIT:collab/src/server/global_state_tests.rs:1597>1500。本次目标的CI release必需gate受阻。允许按现有owner机械拆分测试，不改其他任务docs/collab.md、不删测试、不删空行凑数、不改1500阈值。
允许写：collab/src/server/global_state_tests.rs及一个对应同owner part文件。apply_patch逐文件编辑，保留完整test内容和可见性，避免批量脚本。其他worker写Rust CLI和文档，你的范围互不重叠。不format全仓库，rustfmt仅自有文件。
验证：cargo test --manifest-path collab/Cargo.toml global_state -- --test-threads=4；期望原测试全通过。记录精确命令exit与数量。installed appsdk verify-sdk-source-registry . 验证行数；若遇另一个新文件owner未注册等，只报告不修别的范围。
集成：parent负责最终commit/review/merge/安装。你不commit/push/install/restart，不创建worktree。纯测试机械移动不影响daemon，无需新daemon运行入口。
记录：/Volumes/Intel/playground/appsdk/.worker-runs/requirements-session-lock/baseline/notes.md。先记worker/base/目标，每节点开始记命令日志，完成/失败记证据/下一步。parent异常先读notes核对进程，不盲重跑。最终CLI -o写report.md；parent接收归档再清理。不在产品根写notes/临时脚本，不改全局记忆。
停止：范围内测试失败先定位修正；越界只报原错和拟解。完成合同即回报。
