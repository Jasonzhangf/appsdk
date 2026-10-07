# 首轮 review finding 的事实与升级行为证据

原候选：93851a04e2fa40aaebab26b7b791fcf76a577c8a。Codex首轮controller FAIL；AGY首轮controller PASS。两者原始输出与回执位于 `execution/architecture-*-r1*.json`。不能以AGY PASS覆盖Codex的P1。

## 事实核对和唯一 owner

Codex P1 称 schema 没有声明 requirements_review，并要求把升级后的现代PASS绑定作为schema/migration必需字段。当前 `contracts/records/review-record.schema.json` 已在 `project_bindings.properties.requirements_review` 明确声明 `context_id` 和 `checked`，二者在该block内必需。容器可选是历史兼容的表达；schema同时承载FAIL/UNKNOWN和frozen/retired旧record，并没有当前module stage。不能仅用verdict==pass将binding在全局schema设为必需，否则破坏明确保留的历史记录。

现代PASS的准入由唯一runtime owner `assert_fix_architecture_gate` 校验模块当前stage：非frozen/retired必须存在requirements_review，随后共享context重算拒绝陈旧确认。新producer同样先校验显式ack再进入review identity。schema保证shape，runtime依据真实module stage保证适用性；没有第二需求授权真源，也没有可关闭gate的开关。

0.1.0010→0.1.0011迁移负责SDK/maps/resources，不负责reviewer的判断。`migrate_governance_maps` 保留旧review的identity为历史引用；不改写review原文，也不为旧PASS生成checked或context_id。升级后的现代旧PASS需要新的reviewer明确确认，不能自动映射出一个新PASS。不能通过迁移伪造ack来满足finding。

## 补充公开黑盒

新增 `rust/tests/cli_smoke/part_23.rs` 的 `pin_lock_preserves_review_history_and_requires_current_requirements_ack`，针对installed binary执行公开`pin-lock`、`verify --review-admission`、`review-context`与architecture producer：

1. 旧版本消费者使用project-owned maps和不含requirements_review的现代旧PASS，record identity按旧合同完整绑定；不是坏JSON或随意损坏identity。
2. pin-lock成功升级至0.1.0011，旧review字节保持，迁移历史保留旧review_id；不会自造reviewer确认。
3. 升级后的旧PASS明确失败为`ARCHITECTURE_REQUIREMENTS_REVIEW_MISSING`。
4. 新材料仍可生成；组装后改goal，旧context的确认明确失败为`ARCHITECTURE_REQUIREMENTS_REVIEW_CONTEXT_MISMATCH`；原review仍保持。
5. 明确当前context的测试reviewer输入生成新PASS及不同review_id，公开review-admission成功；已有迁移历史字节不变。

此例隔离review迁移语义，使用project-owned map合同；canonical旧map快照、完整历史AgentTeams及frozen兼容另有原有公开迁移回归。fixture确认仍是测试输入，不证明真实身份认证。

安装后新增例PASS、exit0，日志 `execution/installed-migration-review-green.log.gz`。首次补充测试已走通所有产品断言，只在最后把原始JSON与重新serialize结果比较时因末尾newline失败；作者修正为比较执行前后原始字节，再验证PASS。没有生产/schema/migration逻辑改动，没有放宽binding gate，没有添加自动ack。先前519例继续有效，加本例共520；installed公开场景共8例。最终精确候选仍须正式Codex复审及新增测试的独立review后才能集成。
