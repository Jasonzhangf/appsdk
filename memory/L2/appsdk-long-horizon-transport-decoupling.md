<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-24T14:09:35.456150+00:00","entry_kind":null,"id":"appsdk-long-horizon-transport-decoupling","importance":0,"memory_level":2,"review_evidence":["user-decision:2026-11-24:long-horizon-decoupled-from-transport"],"review_status":"reviewed","rule_control":null,"source_refs":[],"tags":["approved","appsdk","collab","long-horizon","transport"],"updated_at":"2026-11-24T14:19:40.515375+00:00"} -->

# appsdk-long-horizon-transport-decoupling

用户批准（2026-11-24）：长程目标与 transport 解耦。目标是持久业务对象；Rex、tmux、Codex AppServer、Desktop 或其他 transport 只是当前执行与投递载体，不能成为目标所有者或长期准入的先决身份。 Ontology: scenario=创建、恢复、订阅或推进 AppSDK/Collab 长程目标；trigger=目标 owner 已由当前作用域内的合法身份确定并准备绑定提醒或推进工作；allowed_actions=按目标 owner、scope、授权和当前有效绑定选择 transport，在 transport 变化后保留同一目标与业务状态；forbidden_actions=要求特定 transport 才能拥有目标，从 transport 类型推导目标 owner，或把目标生命周期等同于某个 pane/session/socket；state_effect=owner 和 goal 状态保持，transport 绑定/投递状态独立变化；owner=long-horizon goal 与 Collab transport adapter 各自 owner；approval=approved; evidence=用户裁决 2026-11-24、候选 C11-03、当前 Rex memory peer 的 goal subscription 失败观察。代码实现和真实入口验证仍待对应 owner 完成。
<!-- project-memory:end -->
