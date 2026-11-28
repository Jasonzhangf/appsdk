<!-- project-memory:v1 {"category":"lesson","created_at":"2026-11-25T05:45:53.535631+00:00","entry_kind":null,"id":"collab-native-tool-failure-recovery-and-question-routing","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["collab","recovery","tool-routing","user-correction"],"updated_at":"2026-11-25T05:45:53.535631+00:00"} -->

# Collab 原生工具恢复与内部问题路由

2026-11-25 复核 Carol 压缩恢复时的 CUA 误用：压缩前原生 functions.exec 调用成功；压缩后把 FREEFORM 函数工具误传为 JSON 对象，连续出现 SyntaxError: Unexpected token ":"，随后切到 CUA。根因是工具调用格式误用，不是已证实的 daemon/runtime 缺陷。修复格式后仍失败时沿原生通道联系技术 owner；Collab peer 不用 CUA、GUI 或 cua_repl 执行 shell、CLI、日志或 Rex 操作。用户明确的问题路由：技术实现、设计、验收和专业判断交派单 architect；代码问题可直接找实际开发 owner；范围、优先级、owner、资源、权限等项目协调问题交 Jenny。执行 peer 不直接问用户内部问题；只在用户专属事实、新授权或用户硬约束变更时由获授权负责人升级。Skill 文案只提供提示级约束，不证明 daemon 已实施运行时强制。细则已写入中央 Collab Skill 与 AppSDK 分发 Skill。
<!-- project-memory:end -->
