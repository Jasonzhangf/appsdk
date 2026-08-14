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
compile
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
`compiler_digest`，并写入可迁移的 `binary_ref: "project-sdk"`；`pin-lock` 会把外部 binary 复制到项目 `.appsdk/sdk.bin`，不写入机器绝对路径。后续 `compile`、promotion、freeze 都校验该副本 digest。

`compile`、promotion、module promotion、freeze、record graph 和 Active publish 均由 Rust 执行。

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
