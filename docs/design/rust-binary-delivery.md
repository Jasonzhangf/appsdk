# Rust Binary Delivery

AppSDK 的正式交付面是 Rust 原生二进制和版本化文档包。业务项目不携带 AppSDK 源码、另一套运行时、编译器实现或 harness 实现。

## Migration policy

Rust CLI 是唯一正式实现：

```text
Rust binary
  -> version / new / verify / pin-lock
  -> compile / promote / freeze
  -> publish-active
  -> record validation
  -> Active/Protected publish
```

正式交付只包含 Rust binary、contracts、templates、docs 和 Skill；仓库不再包含第二套治理入口或参考实现。

当前 Rust binary 已覆盖：

```text
version
new
verify
verify --review-admission
compile
begin-version
promote
promote-module
freeze
publish-active
```

新项目生成后，用发布版 binary 计算并写入真实锁：

```bash
appsdk pin-lock ./my-app --binary /path/to/appsdk
appsdk verify ./my-app
```

`pin-lock` 将 binary digest 写入 `.appsdk/sdk.lock` 的 `digest` 和
`compiler_digest`，并写入可迁移的 `binary_ref: "project-sdk"`；本地会生成忽略且不可作为执行入口的 `.appsdk/sdk.bin` 见证副本。后续 `verify`、`compile`、promotion、freeze 只执行当前全局 `appsdk`，并校验当前运行 binary、Bundle 和锁定摘要；干净 checkout 不依赖本地见证副本。

`pin-lock` 也是真实 SDK 版本迁移入口。0.1.6 只接受项目 0.1.5 或已是 0.1.6，并要求执行中的 binary 与 `--binary` 字节一致。0.1.5 → 0.1.6 会先校验并快照旧 canonical maps 与 frozen ReviewRecord 哈希，再安装新 Bundle/maps，最后让 lock、project version 同步前进；旧 review 只能经精确 migration record 解析旧 map snapshot。该事务也可恢复此前已写入 0.1.6 version/lock、但尚未迁移 maps 的中断状态。禁止用旧 CLI 给新 binary 写锁，禁止手改版本、review hash 或 migration snapshot。

`compile`、promotion、module promotion、freeze、record graph 和 Active publish 均由 Rust 执行。

`verify --review-admission <project> --module <id>` 是 review 与 delivery commit 前的独立门禁。它要求开发白盒和部署黑盒是两组不重叠的 PASS 证据，并把部署黑盒绑定到准确的 artifact hash、environment、安装/重启 receipt 和公开 entrypoint；源码级调用、mock 或把白盒改标签不能通过。

`begin-version` 是 frozen module 的唯一重新开发入口。它验证并保留旧 Active/Protected/record graph，建立 previous/new version 绑定，再仅重开目标 module。

干净 checkout 缺少被忽略的 generated/Active projection 时，先运行 `rehydrate-frozen`。该入口从 FreezeRecord 推导版本，执行声明 build，并以 freeze/promotion artifact hash 验证后重建 generated、Protected 和 Active；调用方不能提供版本或路径，不能复制其他 worktree 的 artifact。发布由 `.appsdk/transactions/rehydrate-<module>/marker.json` 绑定，post-publish verify 失败后只允许同 hash 继续；完全匹配且已完成的 projection 可幂等验收，任意 unowned partial/mismatch 都 fail-fast。Protected archive 若被 ignore、源码已漂移或哈希不符同样 fail-fast。

## Build

```bash
cargo build --manifest-path rust/Cargo.toml
cargo test --manifest-path rust/Cargo.toml
cargo build --release --manifest-path rust/Cargo.toml
```

产物：

```text
rust/target/debug/appsdk
```

发布版使用 `cargo build --release`，并将二进制 digest 写入项目 `.appsdk/sdk.lock`。文档、contracts、templates、Skill 与二进制使用同一 AppSDK release version。

## Boundary

```text
external appsdk binary + docs
  -> project .appsdk contracts
  -> compiled manifest / verified artifact
  -> project runtime
```

`.appsdk-control/` 仍然只是项目本地忽略的运行态，不是二进制真源；`Protected` 仍不能被描述为 shell 级不可读。
