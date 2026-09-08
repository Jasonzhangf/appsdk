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
produce-lifecycle-records
begin-version
promote
promote-module
freeze
publish-active
```

新项目由 `new`/`init` 直接写入当前 0.1.6 Bundle lock，无需执行
`pin-lock`：

```bash
appsdk init ./my-app
appsdk verify ./my-app
```

`.appsdk/sdk.lock` 绑定项目 SDK 版本、contract schema、Bundle digest、
manifest digest 和 Bundle resource set。它不绑定当前运行 binary；
`digest`、`compiler_digest` 和 `binary_ref` 仅可作为旧迁移留下的历史见证，
不参与 `verify`、`compile`、promotion、rehydrate 或 freeze 准入。干净
checkout 不依赖本地 `.appsdk/sdk.bin`。

`pin-lock` 仅保留为真实 SDK 版本迁移入口。0.1.6 接受项目 0.1.5 或可恢复
的未完成 0.1.6 migration，并要求执行中的 binary 与 `--binary` 字节一致。
0.1.5 → 0.1.6 会先校验并快照旧 canonical maps 与 frozen ReviewRecord
哈希，再安装新 Bundle/maps，最后让 lock、project version 同步前进；旧
review 只能经精确 migration record 解析旧 map snapshot。禁止对已完成
0.1.6 初始化的项目重复使用 `pin-lock` 作为日常准入，也禁止手改版本、
review hash 或 migration snapshot。

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

## Global installation

全局安装只有一个入口：

```bash
scripts/install-global-appsdk.sh
```

脚本从自身位置解析仓库根目录，构建 release binary，解析当前 `cargo` 的 bin
目录，在该目录中完成临时文件校验后原子替换 `appsdk`，再精确清理
`~/.local/bin/appsdk`、`~/.cargo/bin/appsdk`（canonical 位置除外）和
`~/.local/lib/appsdk/*/appsdk` 这些 AppSDK 管理的旧入口。它不会递归清理用户目录，
也不会删除项目 build 产物、`project-memory` 或 Collab binary。

安装脚本幂等执行。它只把 SHA-256 作为最终诊断输出，不使用历史 SHA 白名单作为
运行条件；相同版本必须由同一次 release build 和唯一安装入口产生。脚本不能修改
调用方 shell 的命令缓存，完成后在当前 zsh 执行 `rehash`，bash 执行 `hash -r`。
新 binary 的安装也不会自动重启 AppSDK/Collab daemon；daemon 维护由各自官方维护
命令单独完成。

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
