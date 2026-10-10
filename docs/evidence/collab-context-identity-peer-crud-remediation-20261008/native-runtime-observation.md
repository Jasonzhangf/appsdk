# Native Runtime 能力观察

任务：`collab-context-identity-peer-crud-remediation-20261008`
产品输入：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
执行时间：2026-10-08 America/Los_Angeles
执行入口：`/opt/homebrew/bin/codex` 版本 `0.161.0`。隔离 GCM provider `gpt-5.5`, medium。
产品文件：只读。未接触现行 Collab daemon、master、credential、route 或其他 worker。

## 结论

真实 GCM 工作回合、更新 cwd、关闭单一 thread、关闭后 sibling 继续工作，以及第二独占 AppServer endpoint 恢复已有 thread 均已验证。结果未知后的 thread 创建查证仍未验证。以上只证明当前 Codex Native API，不证明 Collab peer CRUD。

原始回执保存在 Goal 私有运行目录 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/native-owner/`：`o1-receipt.json` 是完整 RPC 记录；`unknown-attempt1-receipt.json` 与 `unknown-receipt.json` 是两次独立结果未知观察。两次已记录的 Native 参数/响应及清理数据来自这些回执。原始调用中的 probe nonce 是一次性测试数据，没有用户凭据值。

## 实际行为

| 能力 | 公开 Native 操作与观察 | 判定 |
|---|---|---|
| 创建和工作 | 独立 project idempotency key 重放返回同一 project id。`thread/start` 返回 threadId/sessionId 和 cwd。线程完成真实 `turn/start` GCM 推理；`thread/read(includeTurns=true)` 读到 completed turn 及 assistant nonce。第二个 thread 也独立完成 | 已验证 |
| 更新 | `thread/settings/update` 的 `cwd` 更新返回成功。第一条无 turns 的 metadata read 暂时显示旧 cwd。随后 turn 实际运行 `/private/tmp/collab-oc-fgq65e6l/consumer-a-updated` 中的 `pwd` 和 marker 读取，assistant 返回更新后目录与 marker nonce；之后 `thread/read` 也显示新 cwd | 已验证。早期 read 是延迟投影，不代表更新未生效 |
| 关闭 | B thread 正运行 `sleep 25` 时调用 `thread/archive`；turn 达到 `interrupted`，`thread/read` 显示 `notLoaded`，loaded list 不再包含 B。A 随后完成另一个 GCM turn | 已验证单一 thread 的真实停止效果和 sibling 存活 |
| endpoint 恢复 | 关闭第一个 owned AppServer 后，第二个独占 endpoint 对同一 thread 调用 `thread/resume`，读回相同 thread/session id、更新后 cwd，并完成真实 turn | 已验证原 Native session 可由另一 endpoint 恢复；不等同于 Collab route/binding 已更新 |
| 创建结果未知 | 发出 `thread/start` 后关闭调用连接，并按唯一 projectId 查询 project/thread；预置调用与重复观察都未得到匹配的 thread 关联。Native `thread/start` 请求无 idempotency key/client operation id；单独 `project/create` 虽按 key 幂等，但不自动提供 thread creation 查询 | 未验证。不能安全重放未知 create，也不能声称空结果证明绝未创建 |
| History 读取 | schema 中 `thread/turns/list` 在本机 API 返回 `list_turns is not supported yet`。本机显式 legacy history 的 `thread/read(includeTurns=true)` 提供实际 turn/items | 该列表方法不支持；legacy read 已验证 |

## 资源与证据边界

Native 探针只在其独占 `/tmp/collab-oc-*` / `/tmp/collab-unknown-*` 中创建自己的 AppServer、短 Unix socket、CODEX_HOME、session 和 consumer。每个 owned PID 的 receipt 记录退出码 0 和 socket removal；临时 root 均由 probe 清除。下游实现和 review 不得复用本次 session id。

child observer 的 sandbox 不能 bind Unix socket；父 controller 在已授权的独占执行环境重新运行实际 Native endpoint probe。原失败与后续成功均保留各自记录，未把 sandbox 错误报告成 host 不支持。

## 对 Collab 合同的影响

Native 可以实际完成线程工作、修改线程工作目录设置、归档并停止一个目标 thread、在另一个 endpoint 恢复同一 session。这些能力可以进入实现候选，但 Collab 仍需自己的 daemon lifecycle owner、scope 和 ID 绑定、创建/更新/关闭 receipt、结果未知处理及 route/binding/grant retirement。A6 的安全 unknown-create 查证仍缺最小可验证合同，相关创建实现不得据本观察直接判通过。
