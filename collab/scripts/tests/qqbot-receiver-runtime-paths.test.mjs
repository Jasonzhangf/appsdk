import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { resolveCollabConfig } from "../qqbot-receiver.mjs";

const execFileAsync = promisify(execFile);
const receiverPath = new URL("../qqbot-receiver.mjs", import.meta.url).pathname;

test("resolveCollabConfig: explicit COLLAB_BIN takes effect", () => {
  const { collab, home } = resolveCollabConfig({
    COLLAB_BIN: "/custom/bin/collab",
    HOME: "/tmp/test-home",
  });
  assert.equal(collab, "/custom/bin/collab");
  assert.equal(home, "/tmp/test-home");
});

test("resolveCollabConfig: default COLLAB_BIN derives from home without hardcoding the machine path", () => {
  const { collab, home } = resolveCollabConfig({ HOME: "/tmp/qqbot-sandbox-home" });
  assert.equal(collab, "/tmp/qqbot-sandbox-home/.cargo/bin/collab");
  assert.equal(home, "/tmp/qqbot-sandbox-home");
  assert.ok(!collab.includes("/Users/fanzhang"));
});

test("resolveCollabConfig: missing home fails explicitly", () => {
  assert.throws(() => resolveCollabConfig({}, () => ""), /HOME_MISSING/);
  assert.throws(() => resolveCollabConfig({ HOME: "" }, () => ""), /HOME_MISSING/);
});

test("resolveCollabConfig: no /Users/fanzhang hardcoding remains in receiver source", () => {
  const src = fs.readFileSync(receiverPath, "utf8");
  assert.ok(!src.includes("/Users/fanzhang"));
});

test("resolveCollabConfig: real node entry honors declared env", async () => {
  const { stdout } = await execFileAsync(
    "node",
    [
      "-e",
      `import { resolveCollabConfig } from ${JSON.stringify(receiverPath)};
       console.log(JSON.stringify(resolveCollabConfig(process.env)));`,
    ],
    {
      env: {
        ...process.env,
        HOME: "/tmp/qqbot-e2e-home",
        COLLAB_BIN: "/custom/bin/collab",
      },
    },
  );
  const result = JSON.parse(stdout.trim());
  assert.equal(result.collab, "/custom/bin/collab");
  assert.equal(result.home, "/tmp/qqbot-e2e-home");
});
