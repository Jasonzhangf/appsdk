import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const helperPath = fileURLToPath(new URL("../qqbot-send.mjs", import.meta.url));
const fixtureRoots = [];

test.after(async () => {
  await Promise.all(
    fixtureRoots.map((root) => fs.rm(root, { recursive: true, force: true })),
  );
});

function runHelper(args, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [helperPath, ...args], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      resolve({ code, signal, stdout, stderr });
    });
    child.stdin.end(input);
  });
}

function parseSingleJson(stdout) {
  assert.equal(stdout.endsWith("\n"), true, "stdout must end with one newline");
  const lines = stdout.trimEnd().split("\n");
  assert.equal(lines.length, 1, "stdout must contain exactly one JSON line");
  return JSON.parse(lines[0]);
}

async function readCalls(capturePath) {
  try {
    const raw = await fs.readFile(capturePath, "utf8");
    return raw
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

async function createFixture({
  credentials = [{ appId: "fixture-app-id", appSecret: "fixture-secret" }],
  target = { scope: "group", targetId: "stale-default-target" },
  send = "accepted",
  receipt = {
    id: "platform-message-1",
    timestamp: 1762646400,
    ext_info: { ref_idx: "7" },
  },
} = {}) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "qqbot-send-helper-"));
  fixtureRoots.push(root);
  const runtimeRoot = path.join(root, "runtime");
  const stateRoot = path.join(root, "state");
  const packageRoot = path.join(
    runtimeRoot,
    "node_modules",
    "@tencent-connect",
    "qqbot-nodejs",
  );
  const capturePath = path.join(root, "calls.jsonl");
  await fs.mkdir(path.join(packageRoot, "dist"), { recursive: true });
  await fs.mkdir(stateRoot, { recursive: true });
  await fs.writeFile(
    path.join(runtimeRoot, "package.json"),
    `${JSON.stringify({ private: true, type: "module" })}\n`,
  );
  await fs.writeFile(
    path.join(packageRoot, "package.json"),
    `${JSON.stringify({
      name: "@tencent-connect/qqbot-nodejs",
      version: "fixture",
      type: "module",
      exports: {
        ".": {
          import: "./dist/index.js",
        },
      },
    })}\n`,
  );
  await fs.writeFile(
    path.join(packageRoot, "dist", "index.js"),
    `
import fs from "node:fs";

const capturePath = ${JSON.stringify(capturePath)};
const sendMode = ${JSON.stringify(send)};
const receipt = ${JSON.stringify(receipt)};

function capture(record) {
  fs.appendFileSync(capturePath, JSON.stringify(record) + "\\n");
}

export class ApiError extends Error {
  constructor(message, httpStatus, apiPath, bizCode) {
    super(message);
    this.name = "ApiError";
    this.httpStatus = httpStatus;
    this.path = apiPath;
    this.bizCode = bizCode;
  }
}

export class QQBot {
  constructor(options) {
    capture({
      kind: "construct",
      appId: options.appId,
      hasLogger: Boolean(options.logger),
    });
  }

  start() {
    capture({ kind: "start" });
    throw new Error("gateway start must not be called");
  }

  async sendText(target, content) {
    capture({ kind: "send", target, content });
    if (sendMode === "accepted") return receipt;
    if (sendMode === "missing-timestamp") return { id: receipt.id };
    if (sendMode === "api-rejected") {
      throw new ApiError(
        "platform rejection fixture-secret",
        400,
        "/v2/users/fixture/messages",
        10001,
      );
    }
    if (sendMode === "network-unknown") {
      const error = new Error("socket failure fixture-secret");
      error.code = "ECONNRESET";
      throw error;
    }
    throw new Error("unknown fixture send mode");
  }
}
`,
  );

  if (credentials !== null) {
    await fs.writeFile(
      path.join(stateRoot, "credentials.json"),
      `${JSON.stringify(credentials)}\n`,
    );
  }
  await fs.writeFile(
    path.join(stateRoot, "target.json"),
    `${JSON.stringify(target)}\n`,
  );
  return { root, runtimeRoot, stateRoot, capturePath };
}

test("accepted receipt sends once to the original route without touching state", async () => {
  const fixture = await createFixture();
  const content = "第一行\n第二行\nemoji \u{1f680} and trailing spaces  ";
  const input = {
    message_id: "collab-message-1",
    scope: "c2c",
    target_id: "original-conversation",
    content,
  };

  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify(input),
  );

  assert.equal(result.code, 0);
  assert.equal(result.signal, null);
  assert.equal(result.stderr, "");
  assert.deepEqual(parseSingleJson(result.stdout), {
    kind: "accepted",
    message_id: input.message_id,
    platform_message_id: "platform-message-1",
    timestamp: "1762646400",
    ref_idx: "7",
  });
  assert.ok(!result.stdout.includes("fixture-secret"));
  assert.ok(!result.stderr.includes("fixture-secret"));

  const calls = await readCalls(fixture.capturePath);
  assert.deepEqual(calls, [
    {
      kind: "construct",
      appId: "fixture-app-id",
      hasLogger: true,
    },
    {
      kind: "send",
      target: { scope: "c2c", targetId: "original-conversation" },
      content,
    },
  ]);
  assert.deepEqual(
    (await fs.readdir(fixture.stateRoot)).sort(),
    ["credentials.json", "target.json"],
  );
  assert.deepEqual(
    JSON.parse(await fs.readFile(path.join(fixture.stateRoot, "target.json"), "utf8")),
    { scope: "group", targetId: "stale-default-target" },
  );
});

test("a receipt without a platform timestamp is uncertain", async () => {
  const fixture = await createFixture({ send: "missing-timestamp" });
  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify({
      message_id: "collab-message-2",
      scope: "group",
      target_id: "group-1",
      content: "reply",
    }),
  );

  const output = parseSingleJson(result.stdout);
  assert.equal(result.code, 0);
  assert.equal(output.kind, "uncertain");
  assert.equal(output.message_id, "collab-message-2");
  assert.equal(output.code, "MISSING_PLATFORM_RECEIPT");
  assert.equal(
    (await readCalls(fixture.capturePath)).filter((call) => call.kind === "send")
      .length,
    1,
  );
});

test("a reliable platform rejection is failed and is not retried", async () => {
  const fixture = await createFixture({ send: "api-rejected" });
  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify({
      message_id: "collab-message-3",
      scope: "c2c",
      target_id: "conversation-3",
      content: "reply",
    }),
  );

  const output = parseSingleJson(result.stdout);
  assert.equal(result.code, 0);
  assert.equal(output.kind, "failed");
  assert.equal(output.code, "QQ_API_REJECTED_400_10001");
  assert.ok(!result.stdout.includes("fixture-secret"));
  assert.equal(
    (await readCalls(fixture.capturePath)).filter((call) => call.kind === "send")
      .length,
    1,
  );
});

test("a transport failure after invocation is uncertain and not retried", async () => {
  const fixture = await createFixture({ send: "network-unknown" });
  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify({
      message_id: "collab-message-4",
      scope: "c2c",
      target_id: "conversation-4",
      content: "reply",
    }),
  );

  const output = parseSingleJson(result.stdout);
  assert.equal(result.code, 0);
  assert.equal(output.kind, "uncertain");
  assert.equal(output.code, "ECONNRESET");
  assert.ok(!result.stdout.includes("fixture-secret"));
  assert.equal(
    (await readCalls(fixture.capturePath)).filter((call) => call.kind === "send")
      .length,
    1,
  );
});

test("invalid input fails before the SDK is loaded or called", async () => {
  const fixture = await createFixture();
  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify({
      message_id: "collab-message-5",
      scope: "channel",
      target_id: "target-5",
      content: "reply",
    }),
  );

  const output = parseSingleJson(result.stdout);
  assert.equal(result.code, 0);
  assert.equal(output.kind, "failed");
  assert.equal(output.message_id, "collab-message-5");
  assert.equal(output.code, "INVALID_INPUT");
  assert.deepEqual(await readCalls(fixture.capturePath), []);
});

test("missing credentials fail before constructing or calling the SDK", async () => {
  const fixture = await createFixture({ credentials: null });
  const result = await runHelper(
    [
      "--runtime-root",
      fixture.runtimeRoot,
      "--state-root",
      fixture.stateRoot,
    ],
    JSON.stringify({
      message_id: "collab-message-6",
      scope: "group",
      target_id: "target-6",
      content: "reply",
    }),
  );

  const output = parseSingleJson(result.stdout);
  assert.equal(result.code, 0);
  assert.equal(output.kind, "failed");
  assert.equal(output.code, "CREDENTIALS_MISSING");
  assert.deepEqual(await readCalls(fixture.capturePath), []);
});
