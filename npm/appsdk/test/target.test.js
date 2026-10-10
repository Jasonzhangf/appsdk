import assert from "node:assert/strict";
import test from "node:test";

import { LauncherError } from "../lib/errors.js";
import { selectTarget } from "../lib/target.js";

test("selects the exact supported platform packages", () => {
  assert.deepEqual(selectTarget({ platform: "darwin", arch: "arm64" }), {
    platform: "darwin",
    arch: "arm64",
    targetTriple: "aarch64-apple-darwin",
    packageName: "@jsonstudio/appsdk-darwin-arm64",
    binaries: {
      appsdk: "bin/appsdk",
      "project-memory": "bin/project-memory",
    },
  });

  assert.deepEqual(
    selectTarget({
      platform: "linux",
      arch: "x64",
      glibcVersionRuntime: "2.35",
    }),
    {
      platform: "linux",
      arch: "x64",
      targetTriple: "x86_64-unknown-linux-gnu",
      packageName: "@jsonstudio/appsdk-linux-x64-gnu",
      libc: "glibc",
      binaries: {
        appsdk: "bin/appsdk",
        "project-memory": "bin/project-memory",
      },
    },
  );

  assert.deepEqual(selectTarget({ platform: "win32", arch: "x64" }), {
    platform: "win32",
    arch: "x64",
    targetTriple: "x86_64-pc-windows-msvc",
    packageName: "@jsonstudio/appsdk-win32-x64-msvc",
    binaries: {
      appsdk: "bin/appsdk.exe",
      "project-memory": "bin/project-memory.exe",
    },
  });
});

test("rejects musl and unknown Linux ABIs", () => {
  for (const glibcVersionRuntime of [undefined, "", null]) {
    assert.throws(
      () =>
        selectTarget({
          platform: "linux",
          arch: "x64",
          glibcVersionRuntime,
        }),
      (error) =>
        error instanceof LauncherError && error.code === "UNSUPPORTED_ABI",
    );
  }
});

test("rejects unsupported platforms and CPUs with typed errors", () => {
  assert.throws(
    () => selectTarget({ platform: "freebsd", arch: "x64" }),
    (error) =>
      error instanceof LauncherError &&
      error.code === "UNSUPPORTED_PLATFORM",
  );

  assert.throws(
    () => selectTarget({ platform: "darwin", arch: "x64" }),
    (error) =>
      error instanceof LauncherError && error.code === "UNSUPPORTED_CPU",
  );

  assert.throws(
    () => selectTarget({ platform: "linux", arch: "arm64" }),
    (error) =>
      error instanceof LauncherError && error.code === "UNSUPPORTED_CPU",
  );
});
