import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";

import { LauncherError } from "../lib/errors.js";
import { runBinary } from "../lib/launcher.js";

const mainPackagePath = "/main/package.json";
const platformPackagePath = "/platform/package.json";
const mainPackage = {
  name: "@jsonstudio/appsdk",
  version: "0.1.14-dev.0",
};
const platformPackage = {
  name: "@jsonstudio/appsdk-darwin-arm64",
  version: mainPackage.version,
};

function fakeProcessObject() {
  const processObject = new EventEmitter();
  processObject.pid = 4242;
  processObject.exitCode = 0;
  return processObject;
}

function fakeChild() {
  const child = new EventEmitter();
  child.kill = () => true;
  return child;
}

function baseOptions(overrides = {}) {
  return {
    platform: "darwin",
    arch: "arm64",
    mainPackagePath,
    resolvePackage: () => platformPackagePath,
    readPackage: (filePath) =>
      filePath === mainPackagePath ? mainPackage : platformPackage,
    stat: () => ({ isFile: () => true }),
    access: () => {},
    processObject: fakeProcessObject(),
    signalProcess: () => {},
    ...overrides,
  };
}

test("forwards arguments and stdio and preserves the child exit status", async () => {
  const child = fakeChild();
  const captured = {};
  const options = baseOptions({
    argv: ["init", "--project-root", "path with spaces", "--", "-x"],
    spawn: (binaryPath, argv, spawnOptions) => {
      captured.binaryPath = binaryPath;
      captured.argv = argv;
      captured.spawnOptions = spawnOptions;
      queueMicrotask(() => child.emit("close", 7, null));
      return child;
    },
  });

  const result = await runBinary("appsdk", options);

  assert.deepEqual(captured, {
    binaryPath: "/platform/bin/appsdk",
    argv: ["init", "--project-root", "path with spaces", "--", "-x"],
    spawnOptions: {
      stdio: "inherit",
      shell: false,
      windowsHide: false,
    },
  });
  assert.deepEqual(result, { code: 7, signal: null });
  assert.equal(options.processObject.exitCode, 7);
});

test("uses the project-memory binary path", async () => {
  const child = fakeChild();
  let capturedBinaryPath;
  const options = baseOptions({
    spawn: (binaryPath) => {
      capturedBinaryPath = binaryPath;
      queueMicrotask(() => child.emit("close", 0, null));
      return child;
    },
  });

  await runBinary("project-memory", options);

  assert.equal(capturedBinaryPath, "/platform/bin/project-memory");
});

test("reports missing optional platform dependencies", async () => {
  const options = baseOptions({
    resolvePackage: () => {
      throw new LauncherError(
        "PLATFORM_PACKAGE_MISSING",
        "optional dependency missing",
      );
    },
  });

  await assert.rejects(
    runBinary("appsdk", options),
    (error) =>
      error instanceof LauncherError &&
      error.code === "PLATFORM_PACKAGE_MISSING",
  );
});

test("reports platform package version mismatches", async () => {
  const options = baseOptions({
    readPackage: (filePath) =>
      filePath === mainPackagePath
        ? mainPackage
        : { ...platformPackage, version: "0.1.14-dev.1" },
  });

  await assert.rejects(
    runBinary("appsdk", options),
    (error) =>
      error instanceof LauncherError &&
      error.code === "PLATFORM_PACKAGE_VERSION_MISMATCH",
  );
});

test("reports missing verified platform binaries", async () => {
  const options = baseOptions({
    stat: () => {
      throw new Error("ENOENT");
    },
  });

  await assert.rejects(
    runBinary("appsdk", options),
    (error) =>
      error instanceof LauncherError &&
      error.code === "PLATFORM_BINARY_MISSING",
  );
});

test("reports spawn failures", async () => {
  const child = fakeChild();
  const spawnError = new Error("permission denied");
  spawnError.code = "EACCES";
  const options = baseOptions({
    spawn: () => {
      queueMicrotask(() => child.emit("error", spawnError));
      return child;
    },
  });

  await assert.rejects(
    runBinary("appsdk", options),
    (error) =>
      error instanceof LauncherError &&
      error.code === "PLATFORM_SPAWN_FAILED" &&
      error.cause === spawnError,
  );
});

test("preserves a child termination signal", async () => {
  const child = fakeChild();
  const signals = [];
  const options = baseOptions({
    signalProcess: (pid, signal) => signals.push([pid, signal]),
    spawn: () => {
      queueMicrotask(() => child.emit("close", null, "SIGTERM"));
      return child;
    },
  });

  const result = await runBinary("appsdk", options);

  assert.deepEqual(signals, [[options.processObject.pid, "SIGTERM"]]);
  assert.deepEqual(result, { code: null, signal: "SIGTERM" });
});

test("forwards parent cancellation to the child", async () => {
  const child = fakeChild();
  const killed = [];
  child.kill = (signal) => {
    killed.push(signal);
    return true;
  };
  const options = baseOptions({
    spawn: () => child,
  });

  const running = runBinary("appsdk", options);
  options.processObject.emit("SIGINT");
  queueMicrotask(() => child.emit("close", 130, null));
  const result = await running;

  assert.deepEqual(killed, ["SIGINT"]);
  assert.deepEqual(result, { code: 130, signal: null });
});
