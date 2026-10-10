DONE。D 范围已实现，未 commit、merge、push、install 或 restart。

**改动**
- [verify.yml](/Users/fanzhang/Documents/github/appsdk/.github/workflows/verify.yml:1)：增加按变更路径选择组件；Rust 定向、完整 AppSDK、DAGPipe、Collab、installer、docs、release 独立作业；缺少基线或未知共享路径自动扩大范围；`workflow_dispatch`/tag 才执行完整 release candidate。
- [install-global-appsdk.sh](/Users/fanzhang/Documents/github/appsdk/scripts/install-global-appsdk.sh:131)：保留版本格式检查，并校验构建二进制与 `rust/release-version` 同源。
- [test-install-global-appsdk.sh](/Users/fanzhang/Documents/github/appsdk/scripts/tests/test-install-global-appsdk.sh:21)：移除 `0.1.0010` 硬编码，期望版本从 `rust/release-version` 读取；原有失败构建、缺 Skill、幂等和无关文件断言保留。
- [README.md](/Users/fanzhang/Documents/github/appsdk/README.md:7)：说明日常选测、release 完整门禁、最小 AppSDK 发布边界和 source tag 正式 installer。

**验证**
- `git diff --check`：PASS
- `bash -n` 两个 installer 脚本：PASS
- YAML 解析、workflow 输出引用和依赖检查：PASS
- selector 以真实 commit range 模拟：PASS
- 按合同未运行 cargo、build、installer 测试或真实安装；这些留给 controller 在组合树执行。

产物：[notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/ci-install/notes.md)、[report.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/ci-install/report.md)、[patch.diff](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/ci-install/patch.diff)。patch sha256：`97fd7862864643ff89343de48c195b86562c22e9f6e8846a331703bac60863a0`。