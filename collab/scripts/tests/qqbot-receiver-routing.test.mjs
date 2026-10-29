import test from "node:test";
import assert from "node:assert/strict";
import {
  buildRoutedBody,
  composeBatchBody,
  extensionForAttachment,
  matchCeoTarget,
  parseLeadingAddress,
  resolveAddress,
  resolveInboundRoute,
} from "../qqbot-receiver.mjs";

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
