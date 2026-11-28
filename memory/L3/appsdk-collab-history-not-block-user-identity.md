<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T04:03:28.603082+00:00","entry_kind":"rule","id":"appsdk-collab-history-not-block-user-identity","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["新绑定更新已有绑定"],"axis":"behavior","axis_id":"collab.identity.init","clause_id":"collab.history-no-block","effects":["context 投影更新"],"evidence":["req-collab-identity"],"forbidden_actions":["以旧历史拒绝初始化","读取 journal 或旧操作记录"],"owner":"identity-map owner","rule":"只用 identity-map 中上一轮绑定；新身份或新绑定更新已有绑定；旧历史不能阻碍用户派发的身份和授权。","scenario":"初始化或恢复身份","scope":"AppSDK Collab 通信与身份模型","state_after":"当前绑定为最新有效映射","state_before":"存在旧绑定或历史","trigger":"collab context"},"revision":1},"source_refs":[],"tags":["collab","identity","recovery","user-authorization"],"updated_at":"2026-11-13T23:21:49.838126+00:00"} -->

# 旧历史不能阻碍用户派发的身份和授权

用户确认（2026-11-06，AppSDK 记忆 agent 逐项讨论）：

1. 不看历史；只使用 `~/.collab/identity-map.json` 中上一轮记住的绑定身份。
2. 新身份或新绑定会更新已有绑定，不拒绝初始化。
3. 旧历史不能阻碍用户派发的身份和授权。
4. 冲突源（旧 pane 锚点阶梯、fail-closed 恢复分支）按被覆盖处理；
   新的 `collab context` + identity-map 模型为当前有效语义。

新绑定更新已有绑定意味着 identity-map 是当前唯一身份载体，不需要也不读取
journal 或旧操作记录。与 `appsdk-collab-no-journal-identity-map` 共同构成
当前身份模型。
<!-- project-memory:end -->
