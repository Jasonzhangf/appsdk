<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T09:48:40.198644+00:00","entry_kind":"rule","id":"appsdk-collab-master-promotion-pane","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["tab 首 pane 升 master"],"axis":"behavior","axis_id":"collab.master.promotion-position","clause_id":"collab.master-pane","effects":["无"],"evidence":["memory appsdk-collab-master-promotion-pane"],"forbidden_actions":["非首 pane 升 master"],"owner":"Collab tm_layout/master grant owner","rule":"只有当前 tab 的第一个 pane 才能升级为 master；overview tab 不受限。","scenario":"升级 master","scope":"AppSDK Collab 通信与身份模型","state_after":"首 pane 为 master","state_before":"pane 为 peer","trigger":"master 升级请求"},"revision":1},"source_refs":[],"tags":["collab","master","overview","pane","promotion"],"updated_at":"2026-11-13T23:21:50.582445+00:00"} -->

# 只有 tab 的第一个 pane 能升级为 master

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 只有当前 tab 的第一个 pane 才能升级为 master。
2. overview 的 tab 不受此限制。
3. README 仍写「所有身份都是 equal peer，master 只在无 live master 时提升」，
   与当前 tab/pane 升级规则不符；更新 README 角色与提升两节时须按本规则。
<!-- project-memory:end -->
