"use strict";
// Read capability stays in this page's memory, never in storage or query URLs.
const capability = location.hash.slice(1);
history.replaceState(null, "", location.pathname);
const byId = id => document.getElementById(id);
const groups = [
  ["待安排", ["pending"]], ["等待接单", ["invited", "assigned"]],
  ["执行中", ["working", "blocked", "waiting", "rework"]],
  ["验证 / 交付", ["verifying", "reviewed", "delivered", "accepted"]],
  ["已集成", ["merged"]], ["已关闭", ["closed", "cancelled"]],
];
const labels = { pending: "待安排", invited: "已邀请", assigned: "历史派单", working: "执行中", blocked: "阻塞", waiting: "等待依赖", rework: "返工", verifying: "验证中", reviewed: "已审查", delivered: "已交付", accepted: "待集成", merged: "已集成", closed: "已关闭", cancelled: "已取消" };
let snapshot = null;
let focusedTask = null;
let ownersKey = "";
function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}
function field(title, value) {
  const wrap = element("div", undefined, "detail-field");
  wrap.append(element("h3", title), element("p", value || "未填写"));
  return wrap;
}
function shellQuote(value) { return "'" + value.replaceAll("'", "'\\''") + "'"; }
function renderDetail(task) {
  byId("detail-title").textContent = task.title;
  const content = byId("detail-content");
  content.replaceChildren();
  const fields = [
    ["任务", task.id], ["Owner / 状态", `${task.owner} · ${labels[task.status] || task.status} · revision ${task.revision}`],
    ["任务说明", task.description], ["下一步", task.next_step],
    ["交付条件", task.delivery_condition], ["测试条件", task.test_condition],
    ["邀请对象", task.invited_peer], ["接单记录", task.last_response],
    ["等待任务", task.blocking_task], ["交付证据", task.delivery_evidence],
    ["审查证据", task.review_evidence], ["集成提交", task.integration_commit],
    ["资源收口", task.cleanup_status], ["更新时间", task.updated_at],
  ];
  for (const [name, value] of fields) if (value) content.append(field(name, value));
  const command = element("code", `collab board update ${shellQuote(task.id)} --expected-revision ${task.revision} --next '下一步'`);
  const hint = field("Owner 更新入口", "仅任务 owner 可以执行；版本变化时先重新读取任务板。完成交付、集成和清理仍使用既有 task 命令。");
  hint.append(command);
  content.append(hint);
}
function openDetail(task) {
  focusedTask = task.id;
  renderDetail(task);
  byId("detail").showModal();
}
function render() {
  if (!snapshot) return;
  byId("project").textContent = snapshot.project || "";
  const members = byId("members");
  members.replaceChildren();
  for (const member of snapshot.workers) {
    const item = element("div", undefined, "member");
    const status = { online: "在线", cold: "未驻留", offline: "离线", unknown: "未知" }[member.status] || member.status;
    item.append(element("strong", member.id), element("span", member.role === "master" ? "Master" : "Peer", "role"));
    item.append(element("small", `${status} · ${member.task_ids.length ? `负责 ${member.task_ids.length} 个任务` : "无执行任务"}${member.invitation_ids.length ? ` · ${member.invitation_ids.length} 个邀请` : ""}`));
    members.append(item);
  }
  if (!snapshot.workers.length) members.append(element("p", "暂无在线注册的公共成员；历史任务仍保留。", "muted"));
  const owners = [...new Set(snapshot.tasks.map(task => task.owner))].sort();
  const key = JSON.stringify(owners);
  if (key !== ownersKey) {
    ownersKey = key;
    const selected = byId("owner").value;
    const options = [element("option", "全部成员")];
    options[0].value = "";
    for (const owner of owners) { const option = element("option", owner); option.value = owner; options.push(option); }
    byId("owner").replaceChildren(...options);
    if (owners.includes(selected)) byId("owner").value = selected;
  }
  const owner = byId("owner").value;
  const query = byId("search").value.trim().toLocaleLowerCase();
  const tasks = snapshot.tasks.filter(task => (!owner || task.owner === owner) && (!query || [task.id, task.title, task.next_step || ""].join(" ").toLocaleLowerCase().includes(query)));
  byId("task-count").textContent = `${tasks.length} / ${snapshot.tasks.length}`;
  byId("empty").hidden = snapshot.tasks.length !== 0;
  const board = byId("board");
  board.replaceChildren();
  const known = new Set(groups.flatMap(([, statuses]) => statuses));
  const sections = tasks.some(task => !known.has(task.status)) ? [...groups, ["其他状态", tasks.filter(task => !known.has(task.status)).map(task => task.status)]] : groups;
  for (const [name, statuses] of sections) {
    const selected = tasks.filter(task => statuses.includes(task.status));
    const column = element("section", undefined, "column");
    column.append(element("h3", `${name} · ${selected.length}`, "column-heading"));
    if (!selected.length) column.append(element("p", "暂无任务", "column-empty"));
    for (const task of selected) {
      const card = element("button", undefined, "task"); card.type = "button";
      card.setAttribute("aria-label", `查看 ${task.title} 的任务详情`);
      const head = element("div", undefined, "task-meta");
      head.append(element("span", task.priority.toUpperCase()), element("span", labels[task.status] || task.status, `status ${task.status}`));
      card.append(head, element("strong", task.title, "task-title"), element("small", task.id, "muted"));
      card.append(element("p", task.next_step || "尚未填写下一步", "next-step"));
      card.append(element("div", task.owner, "owner"));
      if (task.invited_peer) card.append(element("small", `邀请 → ${task.invited_peer}`, "invited-peer"));
      card.addEventListener("click", () => openDetail(task));
      column.append(card);
    }
    board.append(column);
  }
  if (focusedTask && byId("detail").open) {
    const task = snapshot.tasks.find(task => task.id === focusedTask);
    if (task) renderDetail(task); else byId("detail").close();
  }
}
async function readBoard() {
  if (!capability) {
    byId("connection").textContent = "缺少只读访问凭证";
    byId("error").hidden = false;
    byId("error").textContent = "请使用 collab dashboard 输出的完整链接重新打开页面。页面不创建或冒用执行者身份。";
    return;
  }
  try {
    const response = await fetch("/api/board", { headers: { Authorization: `Bearer ${capability}` }, cache: "no-store", credentials: "omit" });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    snapshot = data;
    byId("connection").textContent = "已连接 · 只读";
    byId("connection").className = "connected";
    byId("updated").textContent = `最近读取 ${new Date(data.observed_at).toLocaleTimeString()}`;
    byId("error").hidden = true;
    render();
  } catch (error) {
    byId("connection").textContent = "连接失败 · 数据已过期";
    byId("connection").className = "disconnected";
    byId("error").hidden = false;
    byId("error").textContent = `无法读取当前项目状态：${error.message}。${snapshot ? "下方保留的是上次快照，不代表当前进度。" : "尚未读取到任务数据。"}`;
  } finally {
    // Polling is the declared observer mode, not a fallback transport.
    window.setTimeout(readBoard, 2000);
  }
}
byId("owner").addEventListener("change", render);
byId("search").addEventListener("input", render);
byId("close-detail").addEventListener("click", () => byId("detail").close());
byId("detail").addEventListener("close", () => { focusedTask = null; });
readBoard();
