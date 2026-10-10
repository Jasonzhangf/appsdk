import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  assertCandidateVersion,
  candidateVersion,
  mapSourceVersion,
} from "../lib/version.js";

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
const mainPackagePath = join(repoRoot, "npm/appsdk/package.json");
const mainPackage = JSON.parse(readFileSync(mainPackagePath, "utf8"));
const sourceVersion = readFileSync(
  join(repoRoot, "rust/release-version"),
  "utf8",
).trim();

const platformPackages = [
  {
    directory: "appsdk-darwin-arm64",
    name: "@jsonstudio/appsdk-darwin-arm64",
    os: ["darwin"],
    cpu: ["arm64"],
    files: [
      "bin/appsdk",
      "bin/project-memory",
      "skills/appsdk-project-governance/**",
      "skills/appsdk-migration/**",
      "skills/project-memory/**",
    ],
  },
  {
    directory: "appsdk-linux-x64-gnu",
    name: "@jsonstudio/appsdk-linux-x64-gnu",
    os: ["linux"],
    cpu: ["x64"],
    libc: ["glibc"],
    files: [
      "bin/appsdk",
      "bin/project-memory",
      "skills/appsdk-project-governance/**",
      "skills/appsdk-migration/**",
      "skills/project-memory/**",
    ],
  },
  {
    directory: "appsdk-win32-x64-msvc",
    name: "@jsonstudio/appsdk-win32-x64-msvc",
    os: ["win32"],
    cpu: ["x64"],
    files: [
      "bin/appsdk.exe",
      "bin/project-memory.exe",
      "skills/appsdk-project-governance/**",
      "skills/appsdk-migration/**",
      "skills/project-memory/**",
    ],
  },
];

test("main package is one internally consistent development candidate", () => {
  assert.equal(mainPackage.name, "@jsonstudio/appsdk");
  assert.equal(mainPackage.private, true);
  assert.equal(mainPackage.version, candidateVersion(sourceVersion));
  assert.equal(
    assertCandidateVersion(sourceVersion, mainPackage.version),
    "0.1.14-dev.0",
  );
  assert.equal(mainPackage.appsdk.sourceVersion, sourceVersion);
  assert.equal(
    mainPackage.appsdk.npmVersion,
    mapSourceVersion(sourceVersion),
  );
  assert.equal(mainPackage.appsdk.candidate, true);
  assert.equal(mainPackage.appsdk.artifactStatus, "incomplete-until-M5");
  assert.deepEqual(mainPackage.bin, {
    appsdk: "bin/appsdk.js",
    "project-memory": "bin/project-memory.js",
  });
  assert.equal(mainPackage.engines.node, ">=24.0.0");
  assert.equal(mainPackage.scripts.postinstall, undefined);

  const expectedDependencies = Object.fromEntries(
    platformPackages.map(({ name }) => [name, mainPackage.version]),
  );
  assert.deepEqual(mainPackage.optionalDependencies, expectedDependencies);
});

test("platform manifests pin target restrictions and runtime files", () => {
  for (const expected of platformPackages) {
    const manifestPath = join(
      repoRoot,
      "npm/platforms",
      expected.directory,
      "package.json",
    );
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

    assert.equal(manifest.name, expected.name);
    assert.equal(manifest.version, mainPackage.version);
    assert.equal(manifest.private, true);
    assert.deepEqual(manifest.os, expected.os);
    assert.deepEqual(manifest.cpu, expected.cpu);
    if (expected.libc) {
      assert.deepEqual(manifest.libc, expected.libc);
    } else {
      assert.equal(manifest.libc, undefined);
    }
    assert.deepEqual(manifest.files, expected.files);
    assert.equal(manifest.scripts?.postinstall, undefined);
    assert.equal(manifest.appsdk.artifactStatus, "incomplete-until-M5");
  }
});
