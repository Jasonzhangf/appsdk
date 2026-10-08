# 会话授权需求锁：当前实现合同

本合同按用户最新明确指令修正旧设计中的过强认证前提：

> 用户在会话里提供授权就行了，你不要做无用的校验，我们不要过度校验

旧设计中的生物认证、签名、权限隔离和外部授权服务要求不再适用。用户真实会话授权是规范依据。执行 agent 负责从真实会话保留原文和 source，独立 reviewer 对照会话核对指定变更范围。AppSDK 只检查当前契约所需结构、版本、持久提交和消费绑定。不声称认证自然语言或抵抗同 UID 恶意伪造整个账本。

## 单一 owner 与对象

`rust/src/main/requirements.rs` 是唯一读取、校验和提交 owner。长期唯一记录为项目 `.appsdk/requirements.json`，不依赖 task、daemon、临时缓存或日志。文件含 `schema_version:1`、`project_id`、`version`（总提交数）及 `changes`（按提交顺序保留的原始请求和该需求版本）。当前需求从历史末条派生，不另存镜像或 hash 链。每次原子替换整份文件，同时保存当前版本和历史，不引入新数据库。

公开命令：`appsdk requirements <show|history|apply|verify> [project]`；apply 使用 `--input <json>`。show返回生效条目及总version；history返回完整历史；verify返回`locked`或`not_established`，不自动从goal迁移或初始化保护。apply输入为：

```json
{
  "project_id": "从当前project.json读取的项目身份",
  "request_id": "会话内唯一变更请求",
  "requirement_id": "稳定需求ID",
  "base_version": 0,
  "operation": "create",
  "text": "用户明确确立的需求原文及验收",
  "authorization": {
    "role": "user",
    "source": "真实会话及用户消息的引用",
    "original_text": "用户明确确立或修改的指令原文"
  }
}
```

operation为create/replace/revoke/cancel。首次base_version为0；replace/revoke必须为指定条目的当前版本；replace产生下一版原文，revoke产生下一版撤销事件。撤销后不静默重建，恢复需用户明确replace。cancel在提交前不写任何内容；提交后的撤销必须新请求。相同request_id和完全相同输入返回原回执，同ID不同内容拒绝。没有会话授权原文/来源、actor不是user、错项目、缺条目、错旧版本或不适用operation均拒绝。授权自然语言是否覆盖指定变更由agent/reviewer核对，CLI不通过关键词或相似度伪装判断。

提交复用producer原子写和现有reset互斥锁；锁保证apply与reset不会互相丢历史。写失败保持原文件，忙返回明确可重试结果。读取只检查链内结构/单条版本和request唯一性等契约一致性，不额外计算无消费者摘要。完整同权限改写不在承诺范围。

## 消费与保留

需求生效无TTL。有ledger的任务goal仅新增整数`requirements_version`引用总版本，不复制需求内容。编译、验证、review、生命周期和guidance在现有goal消费边界调用同一owner；缺引用或陈旧则阻断。用户合法变更后必须重新绑定并重验受影响候选和证据，不能更改需求来复用旧PASS。旧项目无ledger时保留原兼容语义；带非零绑定却没有ledger时明确缺失。

接口固定：`read_requirements_if_present(root: &Path) -> Option<Value>`返回已验证原ledger；`assert_requirements_current(root: &Path, goal: &Value)`检查当前引用；`requirement_review_material(root: &Path) -> Value`返回所有生效需求及完整原文/来源/版本/历史。`requirements_cli(args: &mut Peekable<IntoIter<String>>)`处理公开CLI。所有判断归该模块，调用者不复制校验。

review-context从ledger加载全部需求/历史并纳入既有context_id；项目有锁时不使用仅goal的替代内容。正式SDK模板要求reviewer逐条核验会话授权、旧版本、实现与行为证据。沿既有ReviewRecord.requirements_review绑定，不新建review数据或第二提示词真源。

普通init升级不改ledger。reset/fresh-init在既有事务staging内原样保留ledger；失败回滚原文件。保留过程也取得同一reset锁。只保留需求及历史，旧goal或旧PASS不因reset自动重新生效。

## DAG、能力与验收

项目两份graph复用原SESE拓扑，节点语义改为会话来源/范围确认→旧版本→变更决策→原子提交或保持→回执，以及加载ledger→版本一致性→goal引用→SDK review材料→独立证据核验→阶段准入。声明责任映射不是新Runtime Operator。

能力证据：真实用户指令在当前会话；producer已有atomic_write_json；reset已有flock事务；公开CLI及现有cli_smoke consumer能覆盖提交/读取/编译/review/reset；官方安装script和CI既有。无需尚未存在的身份服务作为编码前能力。

T1–T16覆盖首次/消费/缺授权/裸确认字段/真实修改/错scope或版本/不能降验收/升级reset保留/结构漂移和陈旧消费/重复并发/写失败和结果查询/取消撤销/歧义显式不上线/旧项目/陈旧证据/正式review绑定。T4、T9按当前会话信任边界执行，不承诺机器识别编造的user原文或恶意整套同权限篡改。歧义由agent不提交而请求具体用户决定，CLI只处理明确请求。

开发从最新origin/main外置worktree；独立设计PASS后代码；GCM workers按文件分工；作者公开黑盒全通过、适用安装，再独立架构review、CI、集成和自有资源清理。当前继承collab测试文件行数gate已明确，另worker仅机械拆分原测试，不修改运行代码或放宽门禁。
