# Rex 基础函数模块

本目录是 Rex API 的 Python stdlib 实验基础模块，位于独立分支，供后续 daemon 兼容迁移评估。它操作官方 Rex CLI，不用原始 socket、不连默认服务、不写用户现有 session，也不接入 Collab daemon。

## 官方 CLI

```text
/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex --autostart=false
```

本次本机验证使用以下显式服务地址。

```text
unix:///Users/fanzhang/Library/Application%20Support/rex/server.sock
```

`--server` 接受官方 CLI 支持的任意非空服务选择器（unix 地址、host label 或
服务地址）。地址协议解析交给官方 CLI；本模块只做非空校验，不自行解析，
也不在失败时回退本机默认服务。不支持的地址由 CLI 原样报错（如
`unsupported server endpoint scheme "tcp"`），保留为 native_error。

## Graph

调用方的组合操作图使用一个入口 `OperationRequest` 和一个出口 `OperationResult`。
发送函数复用调用方已完成的能力与服务事实预检。

```text
OperationRequest
  -> ValidateRequest
  -> DiscoverCapabilities
  -> ReadServerFactsAtConnectionBoundary
  -> ResolveExactTarget
  -> ExecuteOfficialCli
  -> ClassifyNativeResult
  -> Finish
  -> OperationResult
```

拒绝或取消沿类型错误进入 `Finish`。写结果只分成 `definitely_not_delivered`、`queued`、`unknown`，不把 `queued` 读作 receiver 已读取或 agent 已消费。热点路径不开回边，无自动重试和替代 block。

组合测试图使用一个入口 `TestRequest` 和一个出口 `TestReceipt`。

```text
TestRequest
  -> ReadOnlyPreflight
  -> CreateOwnedReceiverSession
  -> RecordReturnedIds
  -> VerifyOwnedTarget
  -> StartBoundedObservation
  -> ExerciseInputAndQueries
  -> VerifyReceiverAckAndStatus
  -> CleanupOwnedSession
  -> TestReceipt
```

失败和取消都到 `CleanupOwnedSession`。没有创建回执时，清理节点 no-op。清理失败仍到唯一 `TestReceipt`，并标记资源未收口。

## 边界

`queued` 只证明官方 CLI 或 Rex 接受/排队。`test_receiver_ack` 只证明测试 receiver 实际读取到完整字节。真实 Collab 消费仍须 mailbox owner 的消费回执，不能由本模块替代。

`program_status` 的空记录就是无已发布状态，不推断 idle。`process` 和 `pwd` 分别记录 native 来源，终端进程存在不推断 agent busy。

`write_text` 保留 UTF-8 字节、换行、空格、组合字符和引号。Enter 由 `press_enter` 单独提交。`submit` 分别返回文本和 Enter 结果，任一步结果 unknown 时停止后续输入。

## 跨机器边界

`server_status` 保留服务原始 `host_id`（可缺失）与 `instance_id`，用于区分
endpoint、文件域和进程代际。它们不是 Collab agent 身份；同名换会话、服务重启
或地址变化都不据此创建新 Collab 身份。`list_hosts` 只作发现证据，空列表是
合法结果，不消费其 hash，也不自动挑选主机或轮询多个地址。

远端 harness 使用显式 receiver argv JSON 数组与远端 cwd。

```text
python3 -m scripts.rex_api.tests.live_receiver \
  --endpoint REMOTE_ENDPOINT --evidence-dir ABSOLUTE_DIR \
  --receiver-argv '["REMOTE_PYTHON","-u","REMOTE_RECEIVER","--run-id","{run_id}"]' \
  --receiver-cwd REMOTE_CWD --request-timeout 180 --bulk-timeout 180
```

远端 argv/cwd 原样进入 `session.create`，`{run_id}` 只替换本次生成的 run id。
不在本机执行 `exists()`、`resolve()` 或平台绝对路径判断，也不把本机
`sys.executable`、receiver 脚本或 cwd 带给远端。远端缺少 argv 或 cwd 时在创建
session 前失败。实际 cwd 仍须由 receiver ready 回报与远端查询确认，创建接受
不能代替 cwd 验收。receiver 未预置时报告依赖缺失，不复制脚本、不建 tunnel。

SSH Unix socket 转发也使用显式远端 argv/cwd。`--request-timeout` 控制普通调用，
`--bulk-timeout` 控制 schema 发现和长输出读取。两个期限都须为正的有限值。
远端解释器应使用已核实的实际可执行文件。macOS 的 Python 启动器可能重新执行
Framework 内的 Python，进程查询会返回实际程序路径。

指定 endpoint 失败时不回退本机、默认 session 或第二个地址。提交后断线保留
`unknown`，已收事件保留并标 `gap_possible=true`，不自动重发正文、追加 Enter
或补造漏失事件。清理只按本次创建回执的完整 session ID。

## 公开函数

```text
discover(endpoint, timeout_s) -> Capabilities
server_status(endpoint, timeout_s) -> CallResult
list_hosts(endpoint, timeout_s) -> CallResult
resolve_target(target, timeout_s) -> RexTarget
write_text(target, text, timeout_s) -> CallResult
press_enter(target, timeout_s) -> CallResult
submit(target, text, enter, timeout_s) -> SubmitReceipt
capture(target, timeout_s) -> CallResult
process(target, timeout_s) -> CallResult
pwd(target, timeout_s) -> CallResult
program_status(target, timeout_s) -> CallResult
inspect(target, timeout_s) -> CallResult
observe(target, duration_s, cancel_event=None) -> EventBatch
```

## 运行方式

### 离线契约测试

```text
python3 -m unittest discover -s scripts/rex_api/tests -v
```

### 公开模块 CLI

```text
python3 -m scripts.rex_api --endpoint ENDPOINT --timeout 180 [--session FULL_SESSION_ID --block FULL_BLOCK_ID] COMMAND ...
```

`submit` 从 UTF-8 stdin 读正文，保留结尾换行。CLI 不提供任意 shell 命令执行、用户 session 清理、服务启停或任意 API 转发。

### 测试 receiver harness

```text
python3 -m scripts.rex_api.tests.live_receiver --endpoint ENDPOINT --evidence-dir ABSOLUTE_DIR
```

该 harness 只操作本次创建回执返回的完整 session/block。创建结果未知时不按名称关闭，也不扩大清理范围。

## 当前证据状态

离线契约测试覆盖 endpoint 传递/隔离、服务身份读取、远端 argv/cwd 传递、
unknown 不重发、readiness 精确 token 正反、失败路径取证。

本机 live harness 已通过一次端到端运行（unicode/multiline ACK、readiness
marker、working/blocked 事件、Enter、长输出 capture、自有 session 清理）。
`server_status` 与 `list_hosts` 已沿公开 CLI 返回真实结果。

2026-11-08 的真实入口结果如下。

| 环境 | 输入与独立 Enter | working/blocked 状态事件 | 完整长输出 | 自有会话清理 |
|---|---|---|---|---|
| 本机 Mac Studio，Rex API revision 11 | 通过 | 通过 | 通过 | 通过 |
| 用户 MacBook Air，macOS arm64，preview 1853 | 通过 | 通过 | 通过 | 通过 |
| coder1，Ubuntu 24.04 x86-64，snapshot | 通过 | 通过 | 通过 | 通过 |

两台远端通过官方支持的 SSH Unix socket 转发连接。接收 ACK 比较实际字节，
事件流先用精确 marker 确认 readiness，再检查后续状态变化。
远端服务身份与本机不同。测试后已关闭本次创建的完整 session。

跨机器服务重启、连接中断后的恢复，以及真实 agent 消费仍待组合验收。
离线测试已覆盖写后未知、事件断线和取消。其证据范围限于模块合同。

daemon 接入设计与增强清单见 [通信兼容](../../docs/design/rex-collab-compatibility-20261108.md)。
当前产物处于独立基础模块阶段。

原始执行证据保存在本机任务笔记目录，不提交动态 session ID 和输出。
