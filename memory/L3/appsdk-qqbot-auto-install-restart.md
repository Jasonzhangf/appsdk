<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T11:52:18.226399+00:00","entry_kind":"rule","id":"appsdk-qqbot-auto-install-restart","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["批准后安装、重建、重启 QQBot"],"axis":"behavior","axis_id":"delivery.qqbot.lifecycle","clause_id":"proc.qqbot","effects":["无"],"evidence":["~/.agents/AGENTS.md runtime 全局安装逐次批准"],"forbidden_actions":["据此重启或替换其他 daemon"],"owner":"用户与 QQBot owner","rule":"QQBot 接收器获批准后可自动安装、重建与重启；不扩展到其他 daemon。","scenario":"QQBot 接收器安装或重启","scope":"AppSDK 项目治理","state_after":"QQBot 已安装运行","state_before":"QQBot 待安装","trigger":"QQBot 变更"},"revision":1},"source_refs":[],"tags":["approval","autostart","global-rule","install","launchagent","qqbot","restart"],"updated_at":"2026-11-13T23:22:06.351322+00:00"} -->

# QQBot 已获用户批准可自动安装、重建与重启

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. QQBot 接收器与其 LaunchAgent/autostart 已获用户批准，可以自动安装；
   获批准后可以执行重建与重启，不另起独立审批。
2. 需在全局规则增加：QQBot 申请审批后被批准，可以进行安装、重建与重启。
   该批准只覆盖 QQBot 接收器本身，不授权重启或替换其他运行中 daemon。
3. 与“runtime 全局安装逐次批准”并列：QQBot 除外，全局写入仍按逐次批准。
<!-- project-memory:end -->
