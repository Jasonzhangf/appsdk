# 独占 Native AppServer 能力探测

- 稳定入口 /opt/homebrew/bin/codex 0.161.0，独占 CODEX_HOME 与 /tmp/collab-cap-7fvtrq5a/native.sock。
- 原始脚本与 receipt：/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/native-capability/probe.py、receipt.json。
- 启动 → initialize → thread/start → thread/read → thread/archive 全部真实成功；thread cwd 读回等于独占 consumer，status idle。
- owned process PID 78429 已 TERM 并退出 0；线程已 archive；未向生产 thread 发消息。
- 第一次失败只因 app-server 不接受 --profile，原错在 attempt1-server.log/receipt；第二次修合法命令，未切换 transport 或伪造成功。
- 此结果只证明底层 thread 创建/读取/归档 API，不证明 Collab CRUD 或推理/READY 已接通。
- 实际 thread/start 与 thread/read 都返回 sessionId，值为 01a11e3f-fa57-7160-8687-2b9c65acb22b，与此独占 thread id 相同。字段从真实响应读取；无需发明或放宽当前 session 核验合同。
- 初稿因摘要未显示 sessionId 曾误判字段缺失；原始响应复核已纠正。后续 admission/CRUD 必须使用真实返回的 sessionId，不能以摘要遗漏判断底层能力缺失。
- 生产 notification sink / archive owner 仍有拒绝 AppServer 分支，底层 helper 存在不足以证明功能可用。
- 后续真实 consumer 在 /tmp/collab-cap-_ortvdyv/consumer 完成两次 installed collab context：registered=true，worker_id 不变。线程 01a11e43-78c7-7cc3-a7d5-f6698a93ae9f 归档，独占 collab down exit 0，AppServer process exit 0。receipt 归档为 native-public-receipt.json。
- 中间一次 consumer 在 playground 祖先下被 context 的 canonical-root 规则拒绝；保留 attempt3-receipt.json。把临时 consumer 改到独占短 /tmp 项目后成功，未改产品规则。
- 当前证据证明普通 Native 自动登记与重放已有可用主链；不能说批准恢复、CRUD 或 inference/READY 已通过。
- 清理仍需移除本任务短 socket 临时目录和 raw capability home；必要 receipt 已归档，执行过程中保留恢复所需引用。
