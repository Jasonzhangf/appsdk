#!/usr/bin/env node

import { createRequire } from "node:module";
import { promises as fs } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const QQ_SDK_PACKAGE = "@tencent-connect/qqbot-nodejs";
const INPUT_KEYS = ["content", "message_id", "scope", "target_id"];
const silentLogger = {
  info() {},
  warn() {},
  error() {},
  debug() {},
};

class PreflightError extends Error {
  constructor(code, message, messageId = "") {
    super(message);
    this.code = code;
    this.messageId = messageId;
  }
}

function failed(messageId, code, error) {
  return { kind: "failed", message_id: messageId, code, error };
}

function uncertain(messageId, code, error) {
  return { kind: "uncertain", message_id: messageId, code, error };
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (
      (flag !== "--runtime-root" && flag !== "--state-root") ||
      typeof value !== "string" ||
      value.startsWith("--") ||
      Object.hasOwn(values, flag)
    ) {
      throw new PreflightError(
        "INVALID_ARGUMENTS",
        "expected --runtime-root <absolute> and --state-root <absolute>",
      );
    }
    values[flag] = value;
  }
  const runtimeRoot = values["--runtime-root"];
  const stateRoot = values["--state-root"];
  if (
    typeof runtimeRoot !== "string" ||
    typeof stateRoot !== "string" ||
    !path.isAbsolute(runtimeRoot) ||
    !path.isAbsolute(stateRoot)
  ) {
    throw new PreflightError(
      "INVALID_ARGUMENTS",
      "expected --runtime-root <absolute> and --state-root <absolute>",
    );
  }
  return { runtimeRoot, stateRoot };
}

async function readStdin() {
  let raw = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) raw += chunk;
  return raw;
}

async function readInput() {
  const raw = await readStdin();
  let input;
  try {
    input = JSON.parse(raw);
  } catch {
    throw new PreflightError(
      "INVALID_INPUT",
      "stdin must be one typed JSON object",
    );
  }
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw new PreflightError(
      "INVALID_INPUT",
      "stdin must be one typed JSON object",
    );
  }
  const messageId =
    typeof input.message_id === "string" ? input.message_id : "";
  const keys = Object.keys(input).sort();
  if (
    keys.length !== INPUT_KEYS.length ||
    keys.some((key, index) => key !== INPUT_KEYS[index])
  ) {
    throw new PreflightError(
      "INVALID_INPUT",
      "stdin must contain exactly message_id, scope, target_id, content",
      messageId,
    );
  }
  if (
    typeof input.message_id !== "string" ||
    input.message_id.trim().length === 0 ||
    (input.scope !== "c2c" && input.scope !== "group") ||
    typeof input.target_id !== "string" ||
    input.target_id.trim().length === 0 ||
    typeof input.content !== "string" ||
    input.content.trim().length === 0
  ) {
    throw new PreflightError(
      "INVALID_INPUT",
      "typed JSON values are invalid",
      messageId,
    );
  }
  return input;
}

function packageImportTarget(exportsValue) {
  if (typeof exportsValue === "string") return exportsValue;
  if (!exportsValue || typeof exportsValue !== "object") return null;
  for (const condition of ["import", "node", "default"]) {
    if (typeof exportsValue[condition] === "string") {
      return exportsValue[condition];
    }
  }
  return null;
}

async function resolveSdkEntry(runtimeRoot) {
  const requireFromRuntime = createRequire(
    path.join(runtimeRoot, "package.json"),
  );
  try {
    const resolved = requireFromRuntime.resolve(QQ_SDK_PACKAGE, {
      conditions: new Set(["import", "node"]),
    });
    if (typeof resolved === "string" && path.isAbsolute(resolved)) {
      return resolved;
    }
  } catch {
    // The SDK is ESM-only. Read its installed manifest when require conditions
    // cannot select the import export.
  }

  const packageRoot = path.join(
    runtimeRoot,
    "node_modules",
    "@tencent-connect",
    "qqbot-nodejs",
  );
  let manifest;
  try {
    manifest = JSON.parse(
      await fs.readFile(path.join(packageRoot, "package.json"), "utf8"),
    );
  } catch {
    throw new PreflightError(
      "SDK_DEPENDENCY_UNAVAILABLE",
      "QQBot SDK is unavailable in the declared runtime root",
    );
  }
  const exportEntry =
    packageImportTarget(manifest.exports?.["."]) ??
    (typeof manifest.module === "string" ? manifest.module : null) ??
    (typeof manifest.main === "string" ? manifest.main : null);
  if (!exportEntry) {
    throw new PreflightError(
      "SDK_INTERFACE_INVALID",
      "QQBot SDK has no import entry",
    );
  }
  return requireFromRuntime.resolve(path.resolve(packageRoot, exportEntry));
}

async function loadSdk(runtimeRoot) {
  const entry = await resolveSdkEntry(runtimeRoot);
  let sdk;
  try {
    sdk = await import(pathToFileURL(entry).href);
  } catch {
    throw new PreflightError(
      "SDK_DEPENDENCY_UNAVAILABLE",
      "QQBot SDK could not be loaded from the declared runtime root",
    );
  }
  if (typeof sdk.QQBot !== "function") {
    throw new PreflightError(
      "SDK_INTERFACE_INVALID",
      "QQBot SDK does not export QQBot",
    );
  }
  return sdk;
}

function credentialEntry(parsed) {
  const first = Array.isArray(parsed) ? parsed[0] : parsed;
  if (
    first &&
    typeof first === "object" &&
    !("appSecret" in first) &&
    "credentials" in first
  ) {
    return Array.isArray(first.credentials)
      ? first.credentials[0]
      : first.credentials;
  }
  return first;
}

async function loadCredentials(stateRoot) {
  let raw;
  try {
    raw = await fs.readFile(path.join(stateRoot, "credentials.json"), "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") {
      throw new PreflightError(
        "CREDENTIALS_MISSING",
        "QQBot credentials are missing",
      );
    }
    throw new PreflightError(
      "CREDENTIALS_UNREADABLE",
      "QQBot credentials could not be read",
    );
  }

  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new PreflightError(
      "CREDENTIALS_INVALID_JSON",
      "QQBot credentials are not valid JSON",
    );
  }
  const entry = credentialEntry(parsed);
  if (
    !entry ||
    typeof entry !== "object" ||
    typeof entry.appId !== "string" ||
    entry.appId.length === 0 ||
    typeof entry.appSecret !== "string" ||
    entry.appSecret.length === 0
  ) {
    throw new PreflightError(
      "CREDENTIALS_INVALID",
      "QQBot credentials must contain non-empty appId and appSecret",
    );
  }
  return { appId: entry.appId, appSecret: entry.appSecret };
}

function platformScalar(value) {
  return (
    (typeof value === "string" && value.length > 0) ||
    (typeof value === "number" && Number.isFinite(value))
  );
}

function acceptedResult(messageId, receipt) {
  if (
    !receipt ||
    typeof receipt !== "object" ||
    !platformScalar(receipt.id) ||
    !platformScalar(receipt.timestamp)
  ) {
    return uncertain(
      messageId,
      "MISSING_PLATFORM_RECEIPT",
      "SDK result did not include platform id and timestamp",
    );
  }
  const result = {
    kind: "accepted",
    message_id: messageId,
    platform_message_id: String(receipt.id),
    timestamp: String(receipt.timestamp),
  };
  const refIdx = receipt.ext_info?.ref_idx;
  if (refIdx !== undefined && refIdx !== null) result.ref_idx = refIdx;
  return result;
}

function safeErrorCode(value, fallback) {
  const text = typeof value === "number" ? String(value) : value;
  return typeof text === "string" && /^[A-Za-z0-9_.:-]{1,80}$/.test(text)
    ? text
    : fallback;
}

function classifySendFailure(error, sdk, messageId) {
  const isApiError =
    typeof sdk.ApiError === "function" && error instanceof sdk.ApiError;
  const status = isApiError ? Number(error.httpStatus) : null;
  if (
    isApiError &&
    Number.isInteger(status) &&
    status >= 400 &&
    status < 500
  ) {
    const bizCode =
      error.bizCode === undefined ? null : safeErrorCode(error.bizCode, null);
    return failed(
      messageId,
      `QQ_API_REJECTED_${status}${bizCode ? `_${bizCode}` : ""}`,
      "QQ platform rejected the request",
    );
  }
  const code = isApiError
    ? "QQ_TRANSPORT_UNCERTAIN"
    : safeErrorCode(error?.code, "SDK_SEND_UNCERTAIN");
  return uncertain(
    messageId,
    code,
    "QQ delivery is unknown after SDK invocation",
  );
}

async function deliver({ runtimeRoot, stateRoot }, input) {
  const sdk = await loadSdk(runtimeRoot);
  const credentials = await loadCredentials(stateRoot);

  let bot;
  try {
    bot = new sdk.QQBot({ ...credentials, logger: silentLogger });
  } catch {
    throw new PreflightError(
      "SDK_INITIALIZATION_FAILED",
      "QQBot SDK could not be initialized",
    );
  }
  if (!bot || typeof bot.sendText !== "function") {
    throw new PreflightError(
      "SDK_INTERFACE_INVALID",
      "QQBot SDK does not expose sendText",
    );
  }

  try {
    const receipt = await bot.sendText(
      { scope: input.scope, targetId: input.target_id },
      input.content,
    );
    return acceptedResult(input.message_id, receipt);
  } catch (error) {
    return classifySendFailure(error, sdk, input.message_id);
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const input = await readInput().catch((error) => {
    if (error instanceof PreflightError) throw error;
    throw new PreflightError("INPUT_READ_FAILED", "stdin could not be read");
  });
  try {
    return await deliver(args, input);
  } catch (error) {
    if (error instanceof PreflightError) {
      return failed(input.message_id, error.code, error.message);
    }
    return failed(
      input.message_id,
      "HELPER_FAILED",
      "helper failed before sending",
    );
  }
}

let result;
try {
  result = await main();
} catch (error) {
  if (error instanceof PreflightError) {
    result = failed(error.messageId ?? "", error.code, error.message);
  } else {
    result = failed("", "HELPER_FAILED", "helper failed before sending");
  }
}
process.stdout.write(`${JSON.stringify(result)}\n`);
