# 最终候选作者验证

任务 rule-upgrade-audit-20261009，git-bug aabed6a；原基线 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a，以下原始验收记录绑定 release 0.1.0013 的候选阶段。该版本已公开发布于 94db69471a488232b6815cccd9efae4e3ff573e5；后续 0.1.0014 修订验收见末节。

## 完整发布测试

- AppSDK：cargo test --manifest-path rust/Cargo.toml --locked，最终533 passed、0 failed、0 ignored、0 filtered；原始release-rust-final.log，exit0。
- DAGPipe：cargo test --manifest-path dagpipe/Cargo.toml --locked --all-targets，28 passed、0 failed、0 ignored、0 filtered；release-dagpipe.log，exit0。未受最后迁移常量移位影响，复用。
- 共享CI的Collab新test入口：976 passed、0 failed、1既有ignored、0 filtered；ci-collab.log，exit0。没有安装/重启/发布Collab。未把ignored计作通过。
- Rust/DAGpipe既有fmt、JSON/版本一致性、shell语法、workflow YAML检查通过。Collab原无fmtgate，全包历史style差异约680KB；正式workflow保留新增correctness tests，没有新增全包style门禁或批量格式化旧代码。
- Installer事务测试：幂等、失败build/缺Skill不替换、无关文件保留通过；installer-test.log。fake cargo只证明安装事务，真实build和正式install另有证据。

## source → build → install → canonical

- scripts/install-global-appsdk.sh从最终source真实releasebuild并安装AppSDK、project-memory、三个SDK-managed Skills，install-final.log exit0。
- /Users/fanzhang/.cargo/bin/appsdk version = appsdk 0.1.0013 (rust)。
- 两份canonical binary与同source release artifact逐字相同；三个installed Skill目录与sdk-skill-sources逐文件相同。
- canonical verify-sdk-source-registry当前source通过，原1500行阈值保留；迁移常量/历史lookup归已有canonical_map owner，新增测试沿既有include分片骨架放part25。首次sourcegate失败及修复过程保留，不伪报首轮PASS。

## 真实公开行为

canonical_acceptance.py / canonical-receipt.json绑定15个真实CLI命令及各自stdout/stderr/exit：

- 合法0012 consumer由旧canonical0012 new生成。新canonical对普通init明确SDK_VERSION_MIGRATION_REQUIRED拒绝；项目全部文件类型/内容及隔离全局state快照不变。
- 0012 pin-lock成功，project/lock/resources/bundle一致为0013，生成0012→0013记录。用户fixture自有AGENTS、Skill、Active/Protected、record文件逐字保留。
- 迁移后init和verify成功；重复init完整项目树不变。
- 合法0011历史archive经0011→0012→0013连续迁移，verify成功。只断言fixture实际存在的历史记录/ledger；用户需求保留另外由全套中的公开CLI回归验证，不伪称该fixture必有需求ledger。
- 当前new/verify成功。Guidance选用/未选用两种bootstrap均只读，proposal为uncovered_durable_changes_only，提示真实CI/hook审计、已覆盖会话授权复用和可选Guidance。
- 为隔离fixture，PATH不含Collab，APPSDK_HOME/COLLAB_STATE_DIR/GIT_CEILING_DIRECTORIES均指任务owned路径。optional Collab告警保留，没有将其缺失称为Collab live验收。

## CI选择和边界

ci-selector-receipt.json：10种场景使用真实隔离fixture commit差异验证文档、Guidance、Skill、Communication、DAGPipe、Collab、version、unknown、release、缺diff基准。普通局部组件不无条件运行release全套；未知/共享/版本依赖扩大对应包。v* tag或workflow_dispatch跑AppSDK发布范围完整门禁。正式GitHub CI将在交付边界核对，当前不冒称远端CI已通过。

## 根因与消融

旧pin ordinary init的首次偏离已定位：写入前没有pin检查，导致root合同/bundle先刷新，而lock writer保留旧版。canonical0012原复现及init-red-r2行为红测保留；新guard位于任何layout/resource/registry/memory写之前，init仍为初始化owner，pin-lock仍为迁移owner。当前/迁移后成功、旧pin拒绝且零写入、非法worktree拒绝等行为已验收；未提供撤回guard后复现污染再恢复的因果反向干预原件，负向行为检查不代替该证明。

没有新增审批账本、规则库或调度器。删除两个无消费者的过时schema/template副本；root合同与.appsdk分发镜像有不同消费者，rules生成兼容路径与project-owned module-registry保留。Collab/Memory/Communication历史静态疑点按审计报告交各owner，未当成本次已修复或全行为证明。

原始证据唯一目录：/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/validation/。必要记录保留归档，owned consumer/测试fixture在交付后回收。源码源码审计内部partial/unknown见coverage-notes.md，不声明逐分支穷尽。

## 最终artifact SHA-256

## Review finding 的受影响复验

首轮 controller review 为 FAIL，一个 P1：registry-only CI 缺少已有公开注册、init/runtime 及 communication 消费者。已在原 workflow 补齐依赖选择与公开 CLI 测试集合，未改变 binary/Skill 输入。`ci-selector-registry-receipt.json` 以真实 Git commit 范围证明 registry+communication 被选中，全 AppSDK/DAGPipe/Collab/release 未选中。直接提取并执行修改后的 Registry/Communication 两个 workflow run 脚本，`ci-registry-closure.log` exit 0：内部 21、公开 CLI 74、Communication CLI 99、event schema 5、request schema 20，全为 0 failed/0 ignored。其他完整测试、安装和 canonical 证据输入不变而复用。

- appsdk: `700c98cd3c99b16c9a06d914cde55d70f6c45a5f419edebe191850f3d9970c3d`
- project-memory: `a76467cbaafef577b670a2d4881acba0546ce564e7da9584bbbbee834d24a324`

## 0.1.0014 必要规则修订的作者验收

新基线为已公开 0.1.0013 的 `94db69471a488232b6815cccd9efae4e3ff573e5`。独立结案审计的五项已处理：输入变化只刷新受影响证据；实质计划变化才重规划，review 沿宿主唯一合同；默认提示词不强制可选流程；migration 引用 canonical init 合同；reset 示例使用外置 worktree；原 guard 证据主张限定为行为验收。

本补丁追加 0013→0014 的既有迁移机制。0013 maps 从基线保存逐字快照，所有已发布历史迁移 bytes 不变。当前 resource-map 仅更新 SDK 版本描述，新 manifest 的 source/target digests 已核对。现有 canonical_map owner 接收两个常量，未加模块、框架或阈值；main.rs 1496 行，原上限 1500 保留。

作者验收已通过：

- `cargo fmt --manifest-path rust/Cargo.toml -- --check`、候选及 canonical `verify-sdk-source-registry`。
- 56 次实际匹配的定向测试执行：migration 2，pin-lock 35，ordinary-init 4，repeated-init 3，其余相关公开消费者和改变断言的用例 12。一次 helper 名称误选返回零匹配，未计作验收；实际消费者已由 pin-lock 集合覆盖。
- 官方 `scripts/install-global-appsdk.sh` exit 0，两个 canonical binaries 和三个 installed Skills 与最终候选产物逐字一致。`scripts/tests/test-install-global-appsdk.sh` exit 0，包括正常安装、重复安装、失败构建与缺失 Skill source 的事务边界。
- 17 条真实 installed CLI 命令通过：合法 canonical 0013 创建的 consumer 在旧 pin 普通 init 时 typed 拒绝且项目树/隔离 host state 零写入；0013→0014、重复 init、重复 pin、项目自有 AGENTS/Skill/records/Active/Protected 保留；归档的真实 0012 consumer 连续迁移到 0014、既有历史记录逐字保留；新建/verify、当前 pin 不制造迁移记录；Guidance 选用与未选用两种只读 proposal。

本地未重跑无差别 AppSDK/DAGPipe/Collab 全套。未变架构和产品范围沿原审计及证据复用；全量交由新 tag 的 release gate。Collab 不在隔离验收 PATH 中，本补丁不宣称 live Collab 验收。

0.1.0014 canonical SHA-256：appsdk `01c21fee86003acb8676f3672128f0fed328966319ee60dc677bd9685a3ab275`；project-memory `2dfd2b87e43d2cf8de8f870b1dca42415c83e99d37459bbd1b49e9a2eb517d19`。原始命令、输出、脚本及回执在原 run 的 `closeout-validation/`。本节记录作者验收阶段，独立 review、main/remote、新 tag CI 与发布回执由 run 的交付记录独立绑定，不继承 0013 的 PASS。
