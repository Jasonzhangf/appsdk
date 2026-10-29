#!/usr/bin/env node
// QQBot receiver source. It resolves every route from the host-wide
// `collab list`, then sends the original body through `collab sendmessage`.
import fs from "node:fs";
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const execFileAsync = promisify(execFile);
const COLLAB = process.env.COLLAB_BIN ?? "/Users/fanzhang/.cargo/bin/collab";
const HOME = process.env.HOME || "/Users/fanzhang";
const MAX_INBOUND_ATTACHMENT_BYTES = 110 * 1024 * 1024;
const INBOUND_DOWNLOAD_TIMEOUT_MS = 30000;
const positiveDuration = (value, fallback) => {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
};
const MESSAGE_BATCH_WINDOW_MS = positiveDuration(process.env.QQBOT_BATCH_WINDOW_MS, 30000);
// The sliding window starts 30s after the last message. The cap only stops a
// continuous stream from batching forever, so it must exceed the quiet window.
const MESSAGE_BATCH_MAX_WAIT_MS = positiveDuration(process.env.QQBOT_BATCH_MAX_WAIT_MS, 60000);

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
  if (unique.length === 1) {
    const resolved = unique[0];
    return {
      target: {
        ...resolved,
        address: "ceo",
        resolvedAddress: resolved.address ?? null,
        resolvedWorkerId: resolved.worker_id ?? null,
      },
      source: "ceo",
    };
  }
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
    : {
        target: resolved.target,
        source: "ceo-default",
        resolvedAddress: resolved.target.resolvedAddress ?? null,
        resolvedWorkerId: resolved.target.resolvedWorkerId ?? null,
      };
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
  return {
    workerId,
    workspace: status?.ceo?.workspace ?? null,
    name: status?.ceo?.name ?? null,
  };
};

const parseJsonPayload = (value) => {
  const text = String(value ?? "").trim();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
};

const sendCollabMessage = async (args, cwd) => {
  const { stdout, stderr } = await execFileAsync(COLLAB, args, {
    cwd,
    timeout: 20000,
  });
  const payload = parseJsonPayload(stdout) ?? parseJsonPayload(stderr);
  if (payload?.ok === false) {
    throw new Error(payload.error ?? stderr ?? "COLLAB_SEND_FAILED");
  }
  return payload;
};

const deliveryLabel = (address) =>
  String(address ?? "").toLowerCase() === "ceo" ? "CEO" : address;

const sanitizePathSegment = (value, fallback) => {
  const safe = String(value ?? "")
    .trim()
    .replace(/[/\\\0]/g, "_")
    .replace(/[^A-Za-z0-9._-]/g, "_")
    .slice(0, 120);
  return safe || fallback;
};

const extensionForAttachment = (attachment) => {
  const contentType = String(attachment?.content_type ?? "")
    .toLowerCase()
    .split(";")[0]
    .trim();
  const byType = {
    "image/png": ".png",
    "image/jpeg": ".jpg",
    "image/gif": ".gif",
    "image/webp": ".webp",
    "image/bmp": ".bmp",
    "audio/mpeg": ".mp3",
    "audio/mp4": ".m4a",
    "audio/ogg": ".ogg",
    "video/mp4": ".mp4",
    "video/quicktime": ".mov",
    "application/pdf": ".pdf",
    "text/plain": ".txt",
  };
  if (byType[contentType]) return byType[contentType];
  const name = String(attachment?.filename ?? "");
  const ext = path.extname(name);
  if (ext && ext.length > 1 && ext.length <= 16) return ext;
  if (contentType.startsWith("image/")) return ".img";
  if (contentType.startsWith("video/")) return ".video";
  if (contentType.startsWith("audio/")) return ".audio";
  return ".bin";
};

const attachmentPathBase = (attachment, index) => {
  const filename = String(attachment?.filename ?? `attachment-${index}`).trim();
  const safe = sanitizePathSegment(filename || `attachment-${index}`, `attachment-${index}`);
  const knownExt = extensionForAttachment(attachment);
  const ext = path.extname(safe).toLowerCase();
  if (ext && ext === knownExt) return safe.slice(0, -ext.length);
  if (ext && ext.length > 1 && ext.length <= 16) return safe.slice(0, -ext.length);
  return safe;
};

const downloadInboundAttachment = async ({
  attachment,
  stateDir,
  conversationId,
  messageId,
  index,
}) => {
  const filename = String(attachment?.filename ?? `attachment-${index}`).trim();
  const contentType = String(attachment?.content_type ?? "");
  const url = attachment?.url;
  if (typeof url !== "string" || url.length === 0) {
    return { ok: false, filename, contentType, error: "ATTACHMENT_URL_MISSING" };
  }

  const dir = path.join(
    stateDir,
    "attachments",
    sanitizePathSegment(conversationId, "conversation"),
    sanitizePathSegment(messageId, "message"),
  );
  const base = attachmentPathBase(attachment, index);
  const filePath = path.join(dir, `${String(index).padStart(2, "0")}-${base}${extensionForAttachment(attachment)}`);
  const tmpPath = `${filePath}.tmp-${process.pid}-${Date.now()}`;

  try {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const response = await fetch(url, {
      redirect: "follow",
      signal: AbortSignal.timeout(INBOUND_DOWNLOAD_TIMEOUT_MS),
    });
    if (!response.ok || !response.body) {
      throw new Error(`ATTACHMENT_DOWNLOAD_HTTP_${response.status}`);
    }
    let size = 0;
    const source = Readable.fromWeb(response.body);
    source.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_INBOUND_ATTACHMENT_BYTES) {
        source.destroy(new Error("ATTACHMENT_TOO_LARGE"));
      }
    });
    const output = fs.createWriteStream(tmpPath, { flags: "wx", mode: 0o600 });
    await pipeline(source, output);
    if (size === 0) throw new Error("ATTACHMENT_EMPTY");
    await fs.promises.rename(tmpPath, filePath);
    await fs.promises.chmod(filePath, 0o600).catch(() => {});
    return { ok: true, path: filePath, filename, contentType, size };
  } catch (error) {
    await fs.promises.rm(tmpPath, { force: true }).catch(() => {});
    return {
      ok: false,
      filename,
      contentType,
      error: String(error?.message || error).slice(0, 500),
    };
  }
};

const processInboundAttachments = async (
  attachments,
  stateDir,
  conversationId,
  messageId,
) => {
  const saved = [];
  const failed = [];
  if (Array.isArray(attachments)) {
    for (let index = 0; index < attachments.length; index += 1) {
      const result = await downloadInboundAttachment({
        attachment: attachments[index],
        stateDir,
        conversationId,
        messageId,
        index: index + 1,
      });
      if (result.ok) saved.push(result);
      else failed.push(result);
    }
  }
  return { saved, failed };
};

const buildRoutedBody = (text, saved, failed) => {
  const lines = [];
  const trimmed = String(text ?? "").trim();
  if (trimmed) lines.push(trimmed);
  for (const attachment of saved) lines.push(`附件路径：${attachment.path}`);
  for (const attachment of failed) {
    lines.push(`附件失败：${attachment.filename || "附件"}：${attachment.error}`);
  }
  return lines.join("\n").trim();
};

const composeBatchBody = (bodies) =>
  bodies
    .map((body) => String(body ?? "").trim())
    .filter((body) => body.length > 0)
    .join("\n\n");

const statePayload = (value, pid, updatedAt) => ({
  ...value,
  pid,
  updatedAt,
});

async function startReceiver() {
  const runtimeRoot = process.env.QQBOT_RUNTIME_ROOT ?? path.dirname(fileURLToPath(import.meta.url));
  const { QQBot } = await import("@tencent-connect/qqbot-nodejs");
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

  const batches = new Map();
  let batchCounter = 0;
  const appendInboxRecord = (record) =>
    fs.appendFileSync(path.join(state, "inbox.jsonl"), JSON.stringify(record) + "\n", {
      mode: 0o600,
    });
  const appendReceiptRecord = (record) =>
    fs.appendFileSync(path.join(state, "receipts.jsonl"), JSON.stringify(record) + "\n", {
      mode: 0o600,
    });

  const flushBatch = async (batch) => {
    if (!batch || batch.flushing) return;
    batch.flushing = true;
    if (batch.timer) clearTimeout(batch.timer);
    if (batch.maxTimer) clearTimeout(batch.maxTimer);
    batches.delete(batch.key);

    const combinedBody = composeBatchBody(
      batch.messages.map((item) => item.routedBody),
    );
    let deliveryStatus = "failed";
    let deliveryError = null;
    let collabMessageId = null;
    if (combinedBody) {
      try {
        const sent = await sendCollabMessage(
          [
            "sendmessage",
            "--unknown-sender",
            "--subject",
            "QQBot",
            "--to",
            batch.sendToAddress,
            "--content",
            combinedBody,
          ],
          batch.projectRoot,
        );
        collabMessageId = sent?.msg_id ?? sent?.message_id ?? sent?.data?.msg_id ?? null;
        deliveryStatus = "accepted";
      } catch (error) {
        deliveryError = error.stderr || error.message;
      }
    } else {
      deliveryError = "EMPTY_BATCH";
    }

    const deliveredAt = new Date().toISOString();
    for (const item of batch.messages) {
      appendInboxRecord({
        ...item.record,
        batchId: batch.batchId,
        batchSize: batch.messages.length,
        collabMessageId,
        deliveryStatus,
        deliveryError,
        deliveredAt,
      });
    }

    const attachmentCount = batch.messages.reduce(
      (sum, item) => sum + item.attachmentCount,
      0,
    );
    const attachmentErrorCount = batch.messages.reduce(
      (sum, item) => sum + item.attachmentErrorCount,
      0,
    );
    const attachmentNote = attachmentCount > 0 ? `，附件 ${attachmentCount} 个` : "";
    const mergeNote = batch.messages.length > 1 ? `，合并 ${batch.messages.length} 条` : "";
    const confirmation =
      deliveryStatus === "accepted"
        ? `已投递给${batch.routeSource === "explicit" ? "" : "默认"} [${deliveryLabel(batch.deliveryAddress)}]${mergeNote}${attachmentNote}（Collab 已接受）。`
        : `投递失败：${batch.deliveryAddress === "ceo" ? "默认/CEO" : `[${batch.deliveryAddress}]`}，${deliveryError}`;
    try {
      const receipt = await bot.sendText(
        batch.replyTarget,
        `${batch.newlyBound ? "QQ 即时通道已连接，已收到你的消息并完成私聊绑定。" : ""}${confirmation}`,
      );
      appendReceiptRecord({
        at: deliveredAt,
        batchId: batch.batchId,
        inboundIds: batch.messages.map((item) => item.record.messageId),
        batchSize: batch.messages.length,
        deliveryStatus,
        deliveryAddress: batch.deliveryAddress,
        routedTo: batch.deliveryAddress,
        resolvedAddress: batch.resolvedAddress,
        resolvedWorkerId: batch.resolvedWorkerId,
        collabMessageId,
        response: receipt,
        attachments: batch.messages.flatMap((item) => item.attachmentPaths),
        attachmentErrors: batch.messages.flatMap((item) => item.attachmentErrors),
      });
    } catch (error) {
      status({
        event: "batch_confirmation_failed",
        code: error.code ?? "SDK_ERROR",
        connected: true,
        targetBound: true,
        batchId: batch.batchId,
      });
    }
    status({
      event:
        deliveryStatus === "accepted"
          ? "message_batch_received_and_delivered"
          : "local_notification_failed",
      connected: true,
      targetBound: true,
      batchId: batch.batchId,
      batchSize: batch.messages.length,
      routedTo: batch.deliveryAddress,
      deliveryAddress: batch.deliveryAddress,
      resolvedAddress: batch.resolvedAddress,
      resolvedWorkerId: batch.resolvedWorkerId,
      collabMessageId,
      attachmentCount,
      attachmentErrorCount,
    });
  };

  const enqueueBatch = (item, routeInfo) => {
    const key = `${routeInfo.projectRoot}\u0000${routeInfo.sendToAddress}\u0000${routeInfo.sessionTargetId}`;
    let batch = batches.get(key);
    if (!batch) {
      batch = {
        key,
        batchId: `qqbot-batch-${Date.now()}-${++batchCounter}`,
        projectRoot: routeInfo.projectRoot,
        sendToAddress: routeInfo.sendToAddress,
        deliveryAddress: routeInfo.deliveryAddress,
        resolvedAddress: routeInfo.resolvedAddress,
        resolvedWorkerId: routeInfo.resolvedWorkerId,
        sessionTargetId: routeInfo.sessionTargetId,
        routeSource: routeInfo.routeSource,
        messages: [],
        replyTarget: routeInfo.replyTarget,
        newlyBound: false,
        flushing: false,
        timer: null,
        maxTimer: null,
      };
      batches.set(key, batch);
      batch.maxTimer = setTimeout(() => {
        void flushBatch(batch);
      }, MESSAGE_BATCH_MAX_WAIT_MS);
    }
    batch.messages.push(item);
    batch.replyTarget = routeInfo.replyTarget;
    batch.newlyBound = batch.newlyBound || routeInfo.newlyBound;
    batch.routeSource = routeInfo.routeSource;
    if (batch.timer) clearTimeout(batch.timer);
    batch.timer = setTimeout(() => {
      void flushBatch(batch);
    }, MESSAGE_BATCH_WINDOW_MS);
    status({
      event: "message_batched",
      connected: true,
      targetBound: true,
      batchId: batch.batchId,
      batchSize: batch.messages.length,
      deliveryAddress: batch.deliveryAddress,
      routedTo: batch.deliveryAddress,
      resolvedAddress: batch.resolvedAddress,
      resolvedWorkerId: batch.resolvedWorkerId,
    });
  };

  const flushAllBatches = async () => {
    const pending = [...batches.values()];
    await Promise.all(pending.map((batch) => flushBatch(batch)));
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
        const baseBody = leading ? leading.body : String(message.content ?? "");
        let routedBody = "";

        try {
          const attachmentResult = await processInboundAttachments(
            message.attachments,
            state,
            sessionTargetId,
            message.messageId,
          );
          routedBody = buildRoutedBody(
            baseBody,
            attachmentResult.saved,
            attachmentResult.failed,
          );
          if (!routedBody) {
            status({ event: "ignored_empty", connected: true, targetBound: true });
            return;
          }
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
          const resolvedAddress =
            route.resolvedAddress ?? deliveryTarget?.resolvedAddress ?? null;
          const resolvedWorkerId =
            route.resolvedWorkerId ?? deliveryTarget?.resolvedWorkerId ?? null;
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
            resolvedAddress,
            resolvedWorkerId,
            routingError,
          });

          if (!deliveryTarget) {
            const deliveryError = routingError ?? "NO_DELIVERY_TARGET";
            appendInboxRecord({
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
              attachments: [],
              attachmentErrors: [],
              deliveryAddress: null,
              routedTo: null,
              resolvedAddress: null,
              resolvedWorkerId: null,
              collabMessageId: null,
              deliveryStatus: "failed",
              deliveryError,
            });
            try {
              const response = await bot.sendText(
                message.replyTarget,
                `投递失败：${routedAddress ? `[${routedAddress}]` : "默认/CEO"}，${deliveryError}`,
              );
              appendReceiptRecord({
                at: new Date().toISOString(),
                inboundId: message.messageId,
                deliveryStatus: "failed",
                deliveryError,
                response,
              });
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
              connected: true,
              targetBound: true,
              routedAddress: routedAddress ?? null,
              message: deliveryError,
            });
            return;
          }

          enqueueBatch(
            {
              record: {
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
                attachments: attachmentResult.saved.map((attachment) => ({
                  path: attachment.path,
                  filename: attachment.filename,
                  contentType: attachment.contentType,
                  size: attachment.size,
                })),
                attachmentErrors: attachmentResult.failed,
                deliveryAddress,
                routedTo: deliveryAddress,
                resolvedAddress,
                resolvedWorkerId,
              },
              routedBody,
              attachmentCount: attachmentResult.saved.length,
              attachmentErrorCount: attachmentResult.failed.length,
              attachmentPaths: attachmentResult.saved.map(
                (attachment) => attachment.path,
              ),
              attachmentErrors: attachmentResult.failed,
            },
            {
              projectRoot: deliveryTarget.project_root,
              sendToAddress: resolvedAddress ?? deliveryAddress,
              deliveryAddress,
              resolvedAddress,
              resolvedWorkerId,
              sessionTargetId,
              replyTarget: message.replyTarget,
              newlyBound,
              routeSource: route.source,
            },
          );
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

  const recordFatal = (kind, error) => {
    const safe = String(error?.stack || error?.message || error).slice(0, 1200);
    try {
      status({ event: kind, connected: true, targetBound: !!target, error: safe });
    } catch {
      console.error(JSON.stringify({ event: kind, error: safe }));
    }
  };
  process.on("unhandledRejection", (reason) => recordFatal("unhandled_rejection", reason));
  process.on("uncaughtException", (error) => recordFatal("uncaught_exception", error));

  let shuttingDown = false;
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => {
      if (shuttingDown) return;
      shuttingDown = true;
      status({ event: "stopping", connected: false });
      void flushAllBatches()
        .catch((error) => recordFatal("flush_on_shutdown_failed", error))
        .finally(() => {
          try {
            bot.stop();
          } catch (error) {
            recordFatal("stop_failed", error);
          }
        });
    });
  }
  try {
    await bot.start();
  } catch (error) {
    status({ event: "failed", connected: false, code: error.code ?? "SDK_ERROR" });
    process.exitCode = 1;
  }
}

export {
  processInboundAttachments,
  buildRoutedBody,
  extensionForAttachment,
  composeBatchBody,
};

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : null;
const modulePath = path.resolve(fileURLToPath(import.meta.url));
if (invokedPath === modulePath) {
  await startReceiver();
}
