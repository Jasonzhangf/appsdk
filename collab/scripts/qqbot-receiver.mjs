#!/usr/bin/env node
// QQBot receiver source. It resolves every route from the host-wide
// `collab list`, then sends the original body through `collab sendmessage`.
import fs from "node:fs";
import { execFile } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const COLLAB = process.env.COLLAB_BIN ?? "/Users/fanzhang/.cargo/bin/collab";
const HOME = process.env.HOME || "/Users/fanzhang";

export const parseLeadingAddress = (content) => {
  const trimmed = String(content ?? "").trimStart();
  if (!trimmed.startsWith("[")) return null;
  const close = trimmed.indexOf("]");
  if (close <= 1) return null;
  const address = trimmed.slice(1, close).trim();
  if (!address) return null;
  return { address, body: trimmed.slice(close + 1).trimStart() };
};

const uniqueTargets = (matches) => {
  const byKey = new Map();
  for (const entry of matches) {
    byKey.set(
      `${entry.project_root ?? ""}|${entry.worker_id ?? ""}|${entry.address ?? ""}`,
      entry,
    );
  }
  return [...byKey.values()];
};

// Case-insensitive exact resolution against the address published by
// `collab list`. A failed explicit address never falls back to another target.
export const resolveAddress = (requested, targets) => {
  const wanted = String(requested ?? "").trim().toLowerCase();
  if (!wanted) return { error: "ADDRESS_EMPTY" };
  const matches = targets.filter(
    (entry) => String(entry.address ?? "").toLowerCase() === wanted,
  );
  const unique = uniqueTargets(matches);
  if (unique.length === 1) return { target: unique[0] };
  if (unique.length > 1) {
    return {
      error: `ADDRESS_AMBIGUOUS: ${unique
        .map((entry) => entry.address || entry.worker_id)
        .join(", ")}`,
    };
  }
  return { error: `ADDRESS_NOT_FOUND: ${requested}` };
};

export const matchCeoTarget = (targets, ceo) => {
  if (!ceo?.workerId) return { error: "CEO_GRANT_MISSING" };
  let matches = targets.filter((entry) => entry.worker_id === ceo.workerId);
  if (ceo.workspace) {
    const exactWorkspace = matches.filter(
      (entry) =>
        entry.project_root &&
        path.resolve(entry.project_root) === path.resolve(ceo.workspace),
    );
    if (exactWorkspace.length === 0) {
      return { error: `CEO_NOT_IN_DIRECTORY: ${ceo.workerId}@${ceo.workspace}` };
    }
    matches = exactWorkspace;
  }
  const unique = uniqueTargets(matches);
  if (unique.length === 1) return { target: unique[0], source: "ceo" };
  if (unique.length > 1) {
    return { error: `CEO_ADDRESS_AMBIGUOUS: ${ceo.workerId}` };
  }
  return { error: `CEO_NOT_IN_DIRECTORY: ${ceo.workerId}` };
};

export const resolveRequested = async (
  requested,
  targets,
  { lookupCeo },
) => {
  if (String(requested ?? "").trim().toLowerCase() === "ceo") {
    const ceo = await lookupCeo();
    return matchCeoTarget(targets, ceo);
  }
  return resolveAddress(requested, targets);
};

// The explicit branch is authoritative: its failure never calls the default
// lookup. A selected conversation default is also authoritative; only an
// unset default resolves through the current typed CEO grant.
export const resolveInboundRoute = async ({
  leading,
  targets,
  lookupDefault,
  lookupCeo,
}) => {
  if (leading) {
    const resolved = await resolveRequested(leading.address, targets, { lookupCeo });
    return resolved.error
      ? { error: resolved.error, source: null }
      : { target: resolved.target, source: "explicit" };
  }

  const defaultAddress = await lookupDefault();
  if (defaultAddress) {
    const resolved = await resolveRequested(defaultAddress, targets, { lookupCeo });
    return resolved.error
      ? { error: resolved.error, source: null }
      : { target: resolved.target, source: "default" };
  }

  const resolved = await resolveRequested("ceo", targets, { lookupCeo });
  return resolved.error
    ? { error: resolved.error, source: null }
    : { target: resolved.target, source: "ceo-default" };
};

const runCollab = async (args, cwd) => {
  const { stdout } = await execFileAsync(COLLAB, args, { cwd, timeout: 10000 });
  return JSON.parse(stdout);
};

const runCollabRaw = async (args, cwd) => {
  const { stdout } = await execFileAsync(COLLAB, args, { cwd, timeout: 15000 });
  return String(stdout ?? "").trim();
};

const collabList = async () => {
  const parsed = await runCollab(["list"], HOME);
  return (parsed.targets ?? []).filter(
    (entry) =>
      entry.address &&
      entry.deliverable !== "unavailable" &&
      entry.project_root,
  );
};

const conversationDefault = async (cwd, targetId) => {
  if (!targetId) return null;
  const stdout = await runCollabRaw(
    ["default", "--channel", "qqbot", "--conversation", targetId],
    cwd,
  );
  if (!stdout) return null;
  let parsed;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    return stdout;
  }
  const value = parsed.default ?? parsed;
  return typeof value === "string" ? value : (value?.target_address ?? null);
};

const ceoRoute = async (cwd) => {
  const status = await runCollab(["ceo", "status"], cwd);
  const workerId = status?.worker_id ?? status?.ceo?.worker_id ?? null;
  if (!workerId) return null;
  return { workerId, workspace: status?.ceo?.workspace ?? null };
};

const statePayload = (value, pid, updatedAt) => ({
  ...value,
  pid,
  updatedAt,
});

async function startReceiver() {
  const runtimeRoot = process.env.QQBOT_RUNTIME_ROOT ?? path.dirname(fileURLToPath(import.meta.url));
  const require = createRequire(path.join(runtimeRoot, "package.json"));
  const { QQBot } = require("@tencent-connect/qqbot-nodejs");
  const state = process.env.QQBOT_STATE_DIR ?? path.join(runtimeRoot, "state");
  const [credential] = JSON.parse(fs.readFileSync(path.join(state, "credentials.json"), "utf8"));
  const targetPath = path.join(state, "target.json");
  let target = fs.existsSync(targetPath) ? JSON.parse(fs.readFileSync(targetPath, "utf8")) : null;
  const bindUntil = Date.now() + 10 * 60 * 1000;
  const seen = new Set();
  const bot = new QQBot({ appId: credential.appId, appSecret: credential.appSecret });
  const write = (name, value) =>
    fs.writeFileSync(path.join(state, name), JSON.stringify(value), { mode: 0o600 });
  const status = (value) => {
    write("receiver-status.json", statePayload(value, process.pid, new Date().toISOString()));
    console.log(JSON.stringify(value));
  };

  let directoryCache = { at: 0, targets: [] };
  const activeTargets = async (force = false) => {
    if (!force && Date.now() - directoryCache.at < 15000) return directoryCache.targets;
    const targets = await collabList();
    directoryCache = { at: Date.now(), targets };
    return targets;
  };

  bot.on("ready", () => status({ event: "ready", connected: true, targetBound: !!target }));
  bot.on("resumed", () => status({ event: "resumed", connected: true, targetBound: !!target }));
  bot.on("error", (error) =>
    status({ event: "error", code: error.code ?? "SDK_ERROR" }),
  );

  let serial = Promise.resolve();
  bot.on("message", (_ctx, message) => {
    serial = serial
      .then(async () => {
        if (message.kind !== "c2c" || seen.has(message.messageId)) return;
        seen.add(message.messageId);
        let newlyBound = false;
        if (!target) {
          if (Date.now() > bindUntil) {
            status({ event: "binding_window_expired", connected: true });
            return;
          }
          target = { scope: message.replyTarget.scope, targetId: message.replyTarget.targetId };
          write("target.json", target);
          newlyBound = true;
        }
        if (
          message.replyTarget.scope !== target.scope ||
          message.replyTarget.targetId !== target.targetId
        ) {
          return;
        }

        const owner = JSON.parse(
          fs.readFileSync(path.join(state, "channel-owner.json"), "utf8"),
        );
        const sessionTargetId = target?.targetId ?? null;
        const leading = parseLeadingAddress(message.content);
        const routedAddress = leading ? leading.address : null;
        const routedBody = leading ? leading.body : message.content;

        try {
          const targets = await activeTargets();
          const route = await resolveInboundRoute({
            leading,
            targets,
            lookupCeo: () => ceoRoute(owner.owner_project),
            lookupDefault: () =>
              conversationDefault(owner.owner_project, sessionTargetId),
          });
          const deliveryTarget = route.target ?? null;
          const deliveryAddress = deliveryTarget?.address || null;
          const routingError = route.error ?? null;
          status({
            event: "route_debug",
            ownerProject: owner.owner_project,
            sessionTargetId,
            activeCount: targets.length,
            activeAddresses: targets.map((entry) => entry.address || entry.worker_id),
            routeSource: route.source,
            deliveryAddress,
            routedTo: deliveryAddress,
            routingError,
          });

          let deliveryStatus = "failed";
          let deliveryError = routingError;
          if (deliveryTarget) {
            try {
              await execFileAsync(
                COLLAB,
                [
                  "sendmessage",
                  "--unknown-sender",
                  "--to",
                  deliveryAddress,
                  "--content",
                  routedBody,
                ],
                { cwd: deliveryTarget.project_root, timeout: 20000 },
              );
              deliveryStatus = "accepted";
            } catch (error) {
              deliveryError = error.stderr || error.message;
            }
          } else if (!deliveryError) {
            deliveryError = "NO_DELIVERY_TARGET";
          }

          const record = {
            source: "QQBot",
            receivedAt: new Date().toISOString(),
            messageId: message.messageId,
            senderId: message.senderId,
            replyChannel: "QQBot",
            routedOwner: owner.owner_worker_id,
            ownerGeneration: owner.generation,
            content: message.content,
            routedAddress,
            routedBody,
            deliveryAddress,
            routedTo: deliveryAddress,
            deliveryStatus,
            deliveryError,
          };
          fs.appendFileSync(path.join(state, "inbox.jsonl"), JSON.stringify(record) + "\n", {
            mode: 0o600,
          });

          const confirmation =
            deliveryStatus === "accepted"
              ? route.source === "explicit"
                ? `已投递给 [${deliveryAddress}]（Collab 已接受）。`
                : `已投递给默认 [${deliveryAddress}]（Collab 已接受）。`
              : `投递失败：${routedAddress ? `[${routedAddress}]` : "默认/CEO"}，${deliveryError}`;
          const receipt = await bot.sendText(
            message.replyTarget,
            newlyBound
              ? `QQ 即时通道已连接，已收到你的消息并完成私聊绑定。${confirmation}`
              : confirmation,
          );
          fs.appendFileSync(
            path.join(state, "receipts.jsonl"),
            JSON.stringify({
              at: new Date().toISOString(),
              inboundId: message.messageId,
              deliveryStatus,
              deliveryAddress,
              routedTo: deliveryAddress,
              response: receipt,
            }) + "\n",
            { mode: 0o600 },
          );
          status({
            event:
              deliveryStatus === "accepted"
                ? "message_received_and_delivered"
                : "local_notification_failed",
            connected: true,
            targetBound: true,
            routedTo: deliveryAddress,
            deliveryAddress,
          });
        } catch (error) {
          const deliveryError = String(error.stderr || error.message || error).slice(0, 1200);
          const record = {
            source: "QQBot",
            receivedAt: new Date().toISOString(),
            messageId: message.messageId,
            senderId: message.senderId,
            replyChannel: "QQBot",
            routedOwner: owner.owner_worker_id,
            ownerGeneration: owner.generation,
            content: message.content,
            routedAddress,
            routedBody,
            deliveryAddress: null,
            routedTo: null,
            deliveryStatus: "failed",
            deliveryError,
          };
          fs.appendFileSync(path.join(state, "inbox.jsonl"), JSON.stringify(record) + "\n", {
            mode: 0o600,
          });
          try {
            const response = await bot.sendText(
              message.replyTarget,
              `投递失败：${routedAddress ? `[${routedAddress}]` : "默认/CEO"}，${deliveryError}`,
            );
            fs.appendFileSync(
              path.join(state, "receipts.jsonl"),
              JSON.stringify({
                at: new Date().toISOString(),
                inboundId: message.messageId,
                deliveryStatus: "failed",
                response,
              }) + "\n",
              { mode: 0o600 },
            );
          } catch (sendError) {
            status({
              event: "failure_confirmation_failed",
              code: sendError.code ?? "SDK_ERROR",
              connected: true,
              targetBound: true,
            });
          }
          status({
            event: "local_notification_failed",
            code: error.code ?? "NOTIFY_ERROR",
            connected: true,
            targetBound: true,
            routedAddress: routedAddress ?? null,
            message: deliveryError,
          });
        }
      })
      .catch((error) =>
        status({
          event: "message_processing_failed",
          code: error.code ?? "SDK_ERROR",
          connected: true,
          targetBound: !!target,
        }),
      );
    return serial;
  });

  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => {
      status({ event: "stopping", connected: false });
      bot.stop();
    });
  }
  try {
    await bot.start();
  } catch (error) {
    status({ event: "failed", connected: false, code: error.code ?? "SDK_ERROR" });
    process.exitCode = 1;
  }
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : null;
const modulePath = path.resolve(fileURLToPath(import.meta.url));
if (invokedPath === modulePath) {
  await startReceiver();
}
