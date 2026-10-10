**结论**

`F1 CONFIRMED`：ordinary `init` 在合法 `0.1.0011` consumer 上退出 `0`，先写入 `0.1.0012` bundle/resources，同时保留旧 project pin 和旧 `sdk.lock`。后续 `verify` 仍失败。未修改产品源码，未宣称修复、测试 PASS 或独立 review。

**Fixture**

- 来源：`docs/evidence/user-requirement-truth-lock-20261007/session-lock/fixture-appsdk-0011.tar.gz`
- SHA-256：`b72bc517781d91bd609d27dce4f720659e251c6dbb60d720e3c2a1b09a6fcdc1`
- 合法性：archive 内 `.appsdk/project.json` 和 `.appsdk/sdk.lock` 都是 `0.1.0011`；`sdk-resources.json` 记录 100 个 `0.1.0011` resources。
- 运行时：`/Users/fanzhang/.cargo/bin/appsdk`，版本 `0.1.0012`，SHA-256 `f21c33a708cea0a0119690125577e87140c07e0a51af87ad30d5928220469b3b`。
- 环境限制：fixture 位于 linked worktree 下，因此设置 `GIT_CEILING_DIRECTORIES`，使 Git 把 consumer 视为独立目录。为避免 peer 注册或 daemon 启动，隔离 `PATH` 中不含 `collab`；F1 mutation 发生在 optional collab 节点之前。

**复现**

```sh
env APPSDK_HOME="$STATE/appsdk-home" \
  HOME="$STATE/home" \
  COLLAB_STATE_DIR="$STATE/collab-state" \
  GIT_CEILING_DIRECTORIES="$FIXTURE" \
  PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
  /Users/fanzhang/.cargo/bin/appsdk init "$FIXTURE"
```

- `init` exit `0`，stdout 以 `initialized <fixture>` 结束。
- `.appsdk/project.json`：`0.1.0011` -> `0.1.0011`
- `.appsdk/sdk.lock`：`0.1.0011` -> `0.1.0011`
- `.appsdk/sdk-resources.json`：`0.1.0011` -> `0.1.0012`，100 -> 106 resources
- installed bundle manifest：`0.1.0012`
- mutation delta：7 个既有文件被改写，7 个文件新增。
- `init` 后 `verify` 仍为 exit `1`：

```text
PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.0011:required_binary=appsdk-0.1.0011
```

**对照**

- 当前版本 control：`appsdk new` 后运行 ordinary `init`，0 个文件变化，`verify` exit `0`。
- 官方迁移 control：同一合法 `0.1.0011` fixture 的 fresh copy 运行 `appsdk pin-lock ... --binary /Users/fanzhang/.cargo/bin/appsdk`，exit `0`；project、lock、resources 都到 `0.1.0012`，随后 `verify` exit `0`。

**首次偏离**

路径：`init` -> `existing_init_target` -> `init_project` -> `ensure_governance_layout/bootstrap_contracts` -> `install_bundle_resources` -> `write_current_sdk_lock`。

- 第一个可观察 mutation：`bootstrap_contracts` 在 pin 检查前新增当前 root contract `contracts/records/user-requirement-request.schema.json`。
- 主要污染点：`install_bundle_resources` 无条件覆盖当前 `.appsdk` bundle 和 `sdk-resources.json`，见 [governance.rs](/Users/fanzhang/Documents/github/appsdk/rust/src/main/governance.rs:247)。
- 旧版本保留点：`write_current_sdk_lock` 在 project pin 不匹配时提前返回，见 [init.rs](/Users/fanzhang/Documents/github/appsdk/rust/src/main/init.rs:82)。
- 最小修复 owner 建议：`rust/src/main/init.rs::init_project`。在任何 bundle 写入前检查现有 project pin，旧版本 fail closed 并指向 `pin-lock`，或复用 `pin-lock` 的迁移事务。迁移 owner 不应移动。

完整过程与资源清单在 [notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/upgrade-observation/notes.md)。证据保存在 `upgrade-observation/state/`，fixture 保存在 `fixture-old-pin/`、`control-current-pin/`、`fixture-old-pin-pinlock/`、`fixture-old-pin-official/`。产品源码未改，仓库 HEAD 仍为 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`，`git status` 为空。