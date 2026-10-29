import test from "node:test";
import assert from "node:assert/strict";
import {
  buildChannelEnvelope,
  buildRoutedBody,
  composeBatchBody,
  createBatchQueue,
  createInboundSerial,
  extensionForAttachment,
  matchCeoTarget,
  parseLeadingAddress,
  resolveAddress,
  resolveInboundRoute,
} from "../qqbot-receiver.mjs";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const queueRoute = {
  projectRoot: "/projects/appsdk",
  sendToAddress: "appsdk.Bob",
  deliveryAddress: "appsdk.Bob",
  resolvedAddress: "appsdk.Bob",
  resolvedWorkerId: "worker-bob",
  sessionTargetId: "conversation-42",
  replyTarget: { scope: "c2c", targetId: "conversation-42" },
  newlyBound: false,
  routeSource: "default",
};

const targets = [
  {
    address: "appsdk.Bob",
    worker_id: "worker-bob",
    project_root: "/projects/appsdk",
  },
  {
    address: "appsdk.Jenny",
    worker_id: "worker-jenny",
    project_root: "/projects/appsdk",
  },
  {
    address: "ceo",
    worker_id: "worker-ceo",
    project_root: "/Users/fanzhang",
  },
];

test("only a leading bracket selects a route", () => {
  assert.deepEqual(parseLeadingAddress("  [AppSDK.Bob] 原始正文"), {
    address: "AppSDK.Bob",
    body: "原始正文",
  });
  assert.equal(parseLeadingAddress("请转给 [AppSDK.Bob] 处理"), null);
});

test("consecutive messages merge into one body in arrival order", () => {
  assert.equal(
    composeBatchBody(["  first  ", "", "second\nline", "  "]),
    "first\n\nsecond\nline",
  );
});

test("attachment paths and failures stay in the delivered body", () => {
  const body = buildRoutedBody(
    "caption",
    [{ path: "/tmp/image.png" }],
    [{ filename: "broken.jpg", error: "HTTP_404" }],
  );
  assert.equal(
    body,
    "caption\n附件路径：/tmp/image.png\n附件失败：broken.jpg：HTTP_404",
  );
});

test("attachment extensions follow content type before filename", () => {
  assert.equal(
    extensionForAttachment({ content_type: "image/jpeg; charset=binary", filename: "x.png" }),
    ".jpg",
  );
  assert.equal(extensionForAttachment({ filename: "note.md" }), ".md");
  assert.equal(extensionForAttachment({ content_type: "image/webp" }), ".webp");
});

test("addresses match published entries case-insensitively", () => {
  assert.equal(
    resolveAddress("appSDK.bOB", targets).target.worker_id,
    "worker-bob",
  );
});

test("invalid and ambiguous addresses fail without choosing a target", () => {
  assert.match(resolveAddress("appsdk.Noah", targets).error, /ADDRESS_NOT_FOUND/);
  assert.match(
    resolveAddress("APPSDK.BOB", [
      targets[0],
      { ...targets[0], worker_id: "worker-bob-duplicate" },
    ]).error,
    /ADDRESS_AMBIGUOUS/,
  );
});

test("CEO resolution uses the typed grant worker and workspace", () => {
  assert.equal(
    matchCeoTarget(targets, {
      workerId: "worker-ceo",
      workspace: "/Users/fanzhang",
    }).target.worker_id,
    "worker-ceo",
  );
  assert.match(
    matchCeoTarget(targets, {
      workerId: "worker-ceo",
      workspace: "/elsewhere",
    }).error,
    /CEO_NOT_IN_DIRECTORY/,
  );
});

test("an explicit address never reads or falls back to the default", async () => {
  let defaults = 0;
  let ceos = 0;
  const route = await resolveInboundRoute({
    leading: { address: "appsdk.Noah", body: "正文" },
    targets,
    lookupDefault: async () => {
      defaults += 1;
      return "appsdk.Bob";
    },
    lookupCeo: async () => {
      ceos += 1;
      return { workerId: "worker-ceo", workspace: "/Users/fanzhang" };
    },
  });
  assert.match(route.error, /ADDRESS_NOT_FOUND/);
  assert.equal(defaults, 0);
  assert.equal(ceos, 0);
});

test("unset default resolves to CEO and a set default never falls back", async () => {
  const ceo = { workerId: "worker-ceo", workspace: "/Users/fanzhang" };
  const unset = await resolveInboundRoute({
    leading: null,
    targets,
    lookupDefault: async () => null,
    lookupCeo: async () => ceo,
  });
  assert.equal(unset.target.worker_id, "worker-ceo");
  assert.equal(unset.source, "ceo-default");

  let ceos = 0;
  const invalidDefault = await resolveInboundRoute({
    leading: null,
    targets,
    lookupDefault: async () => "appsdk.Missing",
    lookupCeo: async () => {
      ceos += 1;
      return ceo;
    },
  });
  assert.match(invalidDefault.error, /ADDRESS_NOT_FOUND/);
  assert.equal(ceos, 0);
});

test("channel envelope keeps the external reply route and attachments typed", () => {
  assert.deepEqual(
    buildChannelEnvelope({
      batchId: "qqbot-batch-1",
      sessionTargetId: "conversation-42",
      sendToAddress: "appsdk.Bob",
      replyTarget: { scope: "c2c", targetId: "conversation-42" },
      messages: [
        {
          record: {
            senderId: "sender-1",
            messageId: "platform-event-1",
            attachments: [
              { path: "/tmp/a.png", filename: "a.png", contentType: "image/png" },
            ],
          },
        },
        { record: { senderId: "sender-1", messageId: "platform-event-2", attachments: [] } },
      ],
    }),
    {
      source_channel: "qqbot",
      channel_instance: "qqbot",
      external_user: "sender-1",
      external_conversation: "conversation-42",
      platform_message_id: "platform-event-1",
      reply_route: "qqbot://c2c/conversation-42",
      recipient_address: "appsdk.Bob",
      attachments: [
        { local_path: "/tmp/a.png", display_name: "a.png", media_type: "image/png" },
      ],
    },
  );
});

test("an in-flight attachment keeps its open batch from flushing too early", async () => {
  const flushed = [];
  const queue = createBatchQueue({
    windowMs: 20,
    maxWaitMs: 500,
    flush: (batch) => flushed.push(batch),
  });

  queue.enqueue({ routedBody: "text", record: { senderId: "sender-1" } }, queueRoute);
  queue.beginMessage();
  await delay(40);
  assert.equal(flushed.length, 0, "the trailing timer must defer during a download");

  queue.enqueue(
    { routedBody: "image", record: { senderId: "sender-1" } },
    queueRoute,
    Date.now() - 35,
  );
  queue.endMessage();
  await delay(5);
  assert.equal(flushed.length, 1);
  assert.deepEqual(
    flushed[0].messages.map((message) => message.routedBody),
    ["text", "image"],
  );
});

test("a later message extends the quiet deadline instead of splitting the tail", async () => {
  const flushed = [];
  const queue = createBatchQueue({
    windowMs: 40,
    maxWaitMs: 2000,
    flush: (batch) => flushed.push(batch.messages.map((message) => message.routedBody)),
  });

  queue.enqueue({ routedBody: "first", record: { senderId: "sender-1" } }, queueRoute);
  queue.beginMessage();
  await delay(55);

  queue.enqueue({ routedBody: "second", record: { senderId: "sender-1" } }, queueRoute);
  queue.endMessage();
  await delay(10);
  assert.equal(flushed.length, 0, "an extended quiet deadline must not flush early");

  await delay(60);
  assert.deepEqual(flushed, [["first", "second"]]);
});

test("platform message id stays stable across receiver restarts", () => {
  const record = { senderId: "sender-1", messageId: "platform-event-stable" };
  const first = buildChannelEnvelope({
    batchId: "qqbot-batch-100-1",
    sessionTargetId: "conversation-42",
    sendToAddress: "appsdk.Bob",
    replyTarget: { scope: "c2c", targetId: "conversation-42" },
    messages: [{ record }],
  });
  const replay = buildChannelEnvelope({
    batchId: "qqbot-batch-999-7",
    sessionTargetId: "conversation-42",
    sendToAddress: "appsdk.Bob",
    replyTarget: { scope: "c2c", targetId: "conversation-42" },
    messages: [{ record }],
  });
  assert.equal(first.platform_message_id, "platform-event-stable");
  assert.equal(replay.platform_message_id, "platform-event-stable");
  assert.notEqual(first.platform_message_id, first.batchId);
});

test("the trailing window is measured from receipt, not processing completion", async () => {
  const flushed = [];
  const queue = createBatchQueue({
    windowMs: 50,
    maxWaitMs: 500,
    flush: (batch) => flushed.push(batch),
  });
  queue.enqueue(
    { routedBody: "late", record: { senderId: "sender-1" } },
    queueRoute,
    Date.now() - 40,
  );
  await delay(20);
  assert.equal(flushed.length, 1);
});

test("shutdown drain flushes every queued batch without waiting for the window", async () => {
  const flushed = [];
  const queue = createBatchQueue({
    windowMs: 10000,
    maxWaitMs: 20000,
    flush: (batch) => flushed.push(batch),
  });
  queue.enqueue({ routedBody: "a", record: { senderId: "sender-1" } }, queueRoute);
  assert.equal(flushed.length, 0);
  await queue.flushAll();
  assert.equal(flushed.length, 1);
});

test("shutdown drain waits for asynchronous flush completion", async () => {
  let completed = false;
  const queue = createBatchQueue({
    windowMs: 10000,
    maxWaitMs: 20000,
    flush: async () => {
      await delay(25);
      completed = true;
    },
  });
  queue.enqueue({ routedBody: "a", record: { senderId: "sender-1" } }, queueRoute);
  await queue.flushAll();
  assert.equal(completed, true, "flushAll must not resolve before flush finishes");
});

// P1-1: a message received while an earlier attachment download is still
// running must reserve its batch slot at the synchronous callback boundary.
// Otherwise the in-flight counter drops to zero when the download completes and
// the already-received tail is split into a second batch.
test("a message received during a blocked download stays in the same batch", async () => {
  const flushed = [];
  let releaseB;
  const gateB = new Promise((resolve) => {
    releaseB = resolve;
  });
  const queue = createBatchQueue({
    windowMs: 50,
    maxWaitMs: 5000,
    flush: (batch) => flushed.push(batch.messages.map((message) => message.routedBody)),
  });
  const inbound = createInboundSerial({
    admit: () => true,
    reserve: () => queue.beginMessage(),
    release: () => queue.endMessage(),
    process: async (message, receivedAtMs) => {
      if (message.routedBody === "B") await gateB;
      queue.enqueue(
        { routedBody: message.routedBody, record: { senderId: "sender-1" } },
        queueRoute,
        receivedAtMs,
      );
    },
  });

  inbound.handleMessage({ routedBody: "A" }, Date.now());
  await delay(10);
  inbound.handleMessage({ routedBody: "B" }, Date.now()); // reserved, then blocked in the download
  await delay(10);
  inbound.handleMessage({ routedBody: "C" }, Date.now()); // arrives during B and waits behind it
  await delay(70); // A's quiet window elapses while B/C are still reserved

  assert.deepEqual(flushed, [], "the batch must stay open for the reserved tail");

  releaseB();
  await delay(20);

  assert.deepEqual(
    flushed,
    [["A", "B", "C"]],
    "A/B/C were all received inside the window and must merge",
  );
});

// P1-2: a callback admitted before SIGTERM must drain to delivery instead of
// being silently returned because shuttingDown flipped mid-flight.
test("shutdown drains an already-admitted callback instead of dropping it", async () => {
  const flushed = [];
  let releaseA;
  const gateA = new Promise((resolve) => {
    releaseA = resolve;
  });
  const queue = createBatchQueue({
    windowMs: 10000,
    maxWaitMs: 20000,
    flush: (batch) => flushed.push(batch.messages.map((message) => message.routedBody)),
  });
  const inbound = createInboundSerial({
    admit: () => true,
    reserve: () => queue.beginMessage(),
    release: () => queue.endMessage(),
    process: async (message, receivedAtMs) => {
      if (message.routedBody === "A") await gateA;
      queue.enqueue(
        { routedBody: message.routedBody, record: { senderId: "sender-1" } },
        queueRoute,
        receivedAtMs,
      );
    },
  });

  inbound.handleMessage({ routedBody: "A" }, Date.now());
  await delay(10); // A admitted and blocked in its download
  inbound.handleMessage({ routedBody: "B" }, Date.now()); // admitted before shutdown, waiting in serial
  await delay(10);
  const drain = inbound.drain(); // SIGTERM begins after B was admitted
  releaseA();
  await drain;
  await queue.flushAll();

  assert.deepEqual(
    flushed,
    [["A", "B"]],
    "B was received before shutdown and must reach delivery",
  );
});

test("a message that arrives after shutdown is rejected, not queued", async () => {
  const processed = [];
  const rejected = [];
  const queue = createBatchQueue({ windowMs: 10000, maxWaitMs: 20000, flush: () => {} });
  const inbound = createInboundSerial({
    admit: () => true,
    reserve: () => queue.beginMessage(),
    release: () => queue.endMessage(),
    process: async (message) => {
      processed.push(message.routedBody);
    },
    onReject: (message, reason) => rejected.push(reason),
  });

  inbound.drain();
  inbound.handleMessage({ routedBody: "late" });
  await delay(5);

  assert.deepEqual(processed, [], "post-shutdown messages must not be processed");
  assert.deepEqual(rejected, ["SHUTDOWN_IN_PROGRESS"]);
});

test("duplicate messages are ignored at the boundary without reserving", () => {
  const seen = new Set(["dup-1"]);
  let reserved = 0;
  const inbound = createInboundSerial({
    admit: (message) => {
      if (seen.has(message.messageId)) return false;
      seen.add(message.messageId);
      return true;
    },
    reserve: () => {
      reserved += 1;
    },
    release: () => {},
    process: async () => {},
  });

  inbound.handleMessage({ messageId: "dup-1" });

  assert.equal(reserved, 0, "a deduped message must not reserve a batch slot");
});
