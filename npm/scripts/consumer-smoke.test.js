import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, win32 } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { selectTarget } from "../appsdk/lib/target.js";
import {
  ConsumerSmokeError,
  assertCommandSucceeded,
  localShimInvocation,
  npmInvocation,
  readChecksums,
  runCommand,
  runConsumerSmoke,
  selectTarballs,
  verifyInstalledArtifact,
} from "./consumer-smoke.js";
import {
  SUPPORTED_TRIPLES,
  createArchive,
  packageReleaseArtifacts,
  stageReleaseArtifacts,
} from "./release-artifacts.js";

const REAL_NPM_ROOT = fileURLToPath(new URL("../", import.meta.url));
const SOURCE_VERSION = "9.9.0001";
const NPM_VERSION = "9.9.1";
const CANDIDATE_VERSION = "9.9.1-dev.0";
const SOURCE_COMMIT = "a".repeat(40);
const PLATFORM_DIRS = Object.freeze({
  "aarch64-apple-darwin": "appsdk-darwin-arm64",
  "x86_64-unknown-linux-gnu": "appsdk-linux-x64-gnu",
  "x86_64-pc-windows-msvc": "appsdk-win32-x64-msvc",
});
const PLATFORM_RESTRICTIONS = Object.freeze({
  "aarch64-apple-darwin": { os: ["darwin"], cpu: ["arm64"] },
  "x86_64-unknown-linux-gnu": { os: ["linux"], cpu: ["x64"], libc: ["glibc"] },
  "x86_64-pc-windows-msvc": { os: ["win32"], cpu: ["x64"] },
});
const SKILL_NAMES = ["appsdk-project-governance", "appsdk-migration", "project-memory"];
const GOVERNANCE_DIRS = Object.freeze([
  "playground",
  "active/lib",
  "protected/source",
  "generated",
  ".appsdk-control",
]);
const GOVERNANCE_FILES = Object.freeze([
  ".appsdk/sdk-resources.json",
  ".appsdk/contracts/project.schema.json",
  ".appsdk/docs/design/appsdk-project-integration.md",
  ".appsdk/maps/module-registry.json",
  ".appsdk/contracts/records/worktree-record.schema.json",
  ".appsdk/rules/appsdk-project-governance.md",
  ".appsdk/skills/appsdk-project-governance/SKILL.md",
  ".appsdk/skills/appsdk-migration/SKILL.md",
  ".appsdk/skills/project-memory/SKILL.md",
]);

function writeFile(path, contents, mode) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
  if (mode !== undefined) {
    chmodSync(path, mode);
  }
}

function sha256File(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function platformFiles(targetTriple) {
  const suffix = targetTriple === "x86_64-pc-windows-msvc" ? ".exe" : "";
  return [
    `bin/appsdk${suffix}`,
    `bin/project-memory${suffix}`,
    ...SKILL_NAMES.map((name) => `skills/${name}/**`),
  ];
}

function fakeAppsdkScript() {
  return `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const command = process.argv[2] ?? "";
const project = process.argv[3] ?? "";
const dirs = ${JSON.stringify(GOVERNANCE_DIRS)};
const files = ${JSON.stringify(GOVERNANCE_FILES)};
if (command === "version") {
  process.stdout.write("appsdk ${SOURCE_VERSION} (rust)\\n");
} else if (command === "new") {
  if (project === "") {
    process.stderr.write("USAGE: appsdk new [project]\\n");
    process.exitCode = 2;
  } else {
    for (const relativePath of dirs) {
      fs.mkdirSync(path.join(project, relativePath), { recursive: true });
    }
    for (const relativePath of files) {
      const filePath = path.join(project, relativePath);
      fs.mkdirSync(path.dirname(filePath), { recursive: true });
      fs.writeFileSync(filePath, "resource\\n");
    }
    process.stdout.write('appsdk-registration {"sdk_version":"${SOURCE_VERSION}","idempotent":false}\\n');
  }
} else if (command === "verify") {
  if (project === "") {
    process.stderr.write("USAGE: appsdk verify [project]\\n");
    process.exitCode = 2;
  } else {
    let missing = "";
    for (const relativePath of [...dirs, ...files]) {
      if (!fs.existsSync(path.join(project, relativePath))) {
        missing = relativePath;
        break;
      }
    }
    if (missing !== "") {
      process.stderr.write("missing " + missing + "\\n");
      process.exitCode = 1;
    } else {
      process.stdout.write('{"command_ok":true,"development_ready":true}\\n');
    }
  }
} else {
  process.stderr.write("unexpected appsdk command: " + command + "\\n");
  process.exitCode = 2;
}
`;
}

function fakeMemoryScript() {
  return `#!/usr/bin/env node
const command = process.argv[2] ?? "";
if (command === "help" || command === "--help" || command === "-h") {
  process.stdout.write(JSON.stringify({ commands: ["entry", "query", "verify"], query_hint: "project-memory query" }) + "\\n");
} else {
  process.stderr.write("unexpected project-memory command: " + command + "\\n");
  process.exitCode = 2;
}
`;
}

function makeSyntheticRepo(root) {
  const npmRoot = join(root, "npm");
  const skillRoot = join(root, "sdk-skill-sources");
  const optionalDependencies = Object.fromEntries(
    SUPPORTED_TRIPLES.map((triple) => [`@jsonstudio/${PLATFORM_DIRS[triple]}`, CANDIDATE_VERSION]),
  );
  writeFile(join(npmRoot, "appsdk", "package.json"), `${JSON.stringify({
    name: "@jsonstudio/appsdk",
    version: CANDIDATE_VERSION,
    private: true,
    type: "module",
    bin: {
      appsdk: "bin/appsdk.js",
      "project-memory": "bin/project-memory.js",
    },
    files: ["bin/", "lib/", "README.md"],
    engines: { node: ">=24.0.0" },
    optionalDependencies,
    appsdk: {
      sourceVersion: SOURCE_VERSION,
      npmVersion: NPM_VERSION,
      candidate: true,
    },
  }, null, 2)}\n`);
  writeFile(join(npmRoot, "appsdk", "README.md"), "synthetic main package\n");
  cpSync(join(REAL_NPM_ROOT, "appsdk", "bin"), join(npmRoot, "appsdk", "bin"), { recursive: true });
  cpSync(join(REAL_NPM_ROOT, "appsdk", "lib"), join(npmRoot, "appsdk", "lib"), { recursive: true });
  for (const triple of SUPPORTED_TRIPLES) {
    const directory = PLATFORM_DIRS[triple];
    const restrictions = PLATFORM_RESTRICTIONS[triple];
    writeFile(join(npmRoot, "platforms", directory, "package.json"), `${JSON.stringify({
      name: `@jsonstudio/${directory}`,
      version: CANDIDATE_VERSION,
      private: true,
      os: restrictions.os,
      cpu: restrictions.cpu,
      ...(restrictions.libc === undefined ? {} : { libc: restrictions.libc }),
      files: platformFiles(triple),
      appsdk: { targetTriple: triple },
    }, null, 2)}\n`);
    writeFile(join(npmRoot, "platforms", directory, "README.md"), `synthetic ${triple}\n`);
  }
  for (const name of SKILL_NAMES) {
    writeFile(join(skillRoot, name, "SKILL.md"), `---\nname: ${name}\n---\n`);
  }
  return { npmRoot, skillRoot };
}

function fakeBinaries(root) {
  const appsdk = join(root, "fake-appsdk");
  const memory = join(root, "fake-project-memory");
  writeFile(appsdk, fakeAppsdkScript(), 0o755);
  writeFile(memory, fakeMemoryScript(), 0o755);
  return { appsdk, memory };
}

async function buildPackage(root) {
  const { npmRoot, skillRoot } = makeSyntheticRepo(root);
  const { appsdk, memory } = fakeBinaries(root);
  const archiveDir = join(root, "archives");
  mkdirSync(archiveDir, { recursive: true });
  for (const triple of SUPPORTED_TRIPLES) {
    const stageDir = join(root, "stage", triple);
    await stageReleaseArtifacts({
      targetTriple: triple,
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      appsdkBinary: appsdk,
      projectMemoryBinary: memory,
      skillSourceDir: skillRoot,
      stageDir,
    });
    createArchive({ stageDir, outputDir: archiveDir });
  }
  const packageDir = join(root, "package");
  await packageReleaseArtifacts({
    archiveDir,
    outputDir: packageDir,
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: SOURCE_VERSION,
    skillSourceDir: skillRoot,
    npmRoot,
  });
  return { packageDir, npmRoot, skillRoot };
}

function tempRoot(t) {
  const root = mkdtempSync(join(tmpdir(), "consumer-smoke-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test(
  "installs the exact local tarballs and runs the native consumer contract",
  { skip: process.platform === "win32" ? "synthetic fake binaries are POSIX scripts" : false },
  async (t) => {
    const root = tempRoot(t);
    const { packageDir } = await buildPackage(root);
    const target = selectTarget();
    const result = runConsumerSmoke({ packageDir, targetTriple: target.targetTriple, baseDir: root });
    assert.equal(result.targetTriple, target.targetTriple);
    assert.equal(result.appsdkVersion, `appsdk ${SOURCE_VERSION} (rust)`);
  },
);

test("verifies every installed platform runtime file against artifact.json", (t) => {
  const root = tempRoot(t);
  const target = selectTarget();
  const platformDir = join(root, "node_modules", "@jsonstudio", target.packageName.split("/")[1]);
  for (const skillName of SKILL_NAMES) {
    mkdirSync(join(platformDir, "skills", skillName), { recursive: true });
  }
  const binaryPath = join(platformDir, ...target.binaries.appsdk.split("/"));
  writeFile(binaryPath, "synthetic binary\n", 0o755);
  const artifact = {
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: SOURCE_VERSION,
    targetTriple: target.targetTriple,
    files: {
      [target.binaries.appsdk]: sha256File(binaryPath),
    },
  };
  writeFile(join(platformDir, "artifact.json"), `${JSON.stringify(artifact, null, 2)}\n`);

  const verified = verifyInstalledArtifact(platformDir, {
    targetTriple: target.targetTriple,
    sourceVersion: SOURCE_VERSION,
  });
  assert.equal(verified.files[target.binaries.appsdk], artifact.files[target.binaries.appsdk]);

  writeFile(binaryPath, "corrupted binary\n", 0o755);
  assert.throws(
    () => verifyInstalledArtifact(platformDir, { targetTriple: target.targetTriple }),
    (error) => error instanceof ConsumerSmokeError && error.code === "INSTALLED_HASH_MISMATCH",
  );

  writeFile(binaryPath, "synthetic binary\n", 0o755);
  writeFile(join(platformDir, "bin", "extra"), "extra\n", 0o755);
  assert.throws(
    () => verifyInstalledArtifact(platformDir, { targetTriple: target.targetTriple }),
    (error) => error instanceof ConsumerSmokeError && error.code === "INSTALLED_FILE_SET_MISMATCH",
  );
});

test("selects npm-cli.js under the node installation on Windows without npm_execpath", () => {
  const execPath = win32.join("C:\\", "nodejs", "node.exe");
  const npmCli = win32.join("C:\\", "nodejs", "node_modules", "npm", "bin", "npm-cli.js");
  const previous = process.env.npm_execpath;
  process.env.npm_execpath = win32.join("C:\\", "bad", "npm.cmd");
  try {
    const invocation = npmInvocation({
      platform: "win32",
      execPath,
      fileExists: (candidate) => candidate === npmCli,
    });
    assert.deepEqual(invocation, { command: execPath, args: [npmCli] });
    assert.doesNotMatch(JSON.stringify(invocation), /npm\.cmd/);
  } finally {
    if (previous === undefined) {
      delete process.env.npm_execpath;
    } else {
      process.env.npm_execpath = previous;
    }
  }
});

test("routes Windows npm shims through cmd.exe with quoted arguments", () => {
  const projectDir = win32.join("C:\\", "tmp", "consumer project");
  const invocation = localShimInvocation(
    projectDir,
    "appsdk",
    ["new", win32.join("C:\\", "tmp", "governed project")],
    { platform: "win32", comspec: "cmd.exe" },
  );
  assert.equal(invocation.command, "cmd.exe");
  assert.deepEqual(invocation.args.slice(0, 3), ["/d", "/s", "/c"]);
  assert.equal(invocation.args.length, 4);
  assert.match(invocation.args[3], /appsdk\.cmd/);
  assert.match(invocation.args[3], /governed project/);
  assert.match(invocation.args[3], /^".*"$/);
});

test("preserves subprocess launch errors, status, and stderr", (t) => {
  const root = tempRoot(t);
  const missing = runCommand(join(root, "missing-command"), [], { cwd: root, env: process.env });
  assert.equal(missing.error?.code, "ENOENT");
  assert.throws(
    () => assertCommandSucceeded(missing, "FIXTURE_LAUNCH_FAILED", "missing fixture"),
    (error) => error instanceof ConsumerSmokeError
      && error.code === "FIXTURE_LAUNCH_FAILED"
      && error.message.includes("ENOENT")
      && error.message.includes(missing.error.message),
  );

  const failed = runCommand(
    process.execPath,
    ["-e", "process.stderr.write('boom\\n'); process.exit(7)"],
    { cwd: root, env: process.env },
  );
  assert.equal(failed.status, 7);
  assert.throws(
    () => assertCommandSucceeded(failed, "FIXTURE_STATUS_FAILED", "status fixture"),
    (error) => error instanceof ConsumerSmokeError
      && error.code === "FIXTURE_STATUS_FAILED"
      && error.message.includes("status=7")
      && error.message.includes("boom"),
  );
});

test("rejects a tarball whose bytes no longer match SHA256SUMS", async (t) => {
  const root = tempRoot(t);
  const { packageDir } = await buildPackage(root);
  const target = selectTarget();
  const [first] = readdirSync(packageDir).filter((name) => name.endsWith(".tgz")).sort();
  appendFileSync(join(packageDir, first), "corruption");
  assert.throws(
    () => selectTarballs(packageDir, target),
    (error) => error instanceof ConsumerSmokeError && error.code === "CHECKSUM_MISMATCH",
  );
});

test("rejects a package directory that does not contain exactly four tarballs", async (t) => {
  const root = tempRoot(t);
  const { packageDir } = await buildPackage(root);
  writeFileSync(join(packageDir, "unexpected.tgz"), "not a tarball\n");
  assert.throws(
    () => selectTarballs(packageDir, selectTarget()),
    (error) => error instanceof ConsumerSmokeError && error.code === "TARBALL_SET_MISMATCH",
  );
});

test("rejects a malformed SHA256SUMS entry", async (t) => {
  const root = tempRoot(t);
  const { packageDir } = await buildPackage(root);
  writeFileSync(join(packageDir, "SHA256SUMS"), "not-a-hash  file.tgz\n");
  assert.throws(
    () => readChecksums(packageDir),
    (error) => error instanceof ConsumerSmokeError && error.code === "CHECKSUMS_INVALID",
  );
});

test("rejects a requested target that is not the native host", async (t) => {
  const root = tempRoot(t);
  const { packageDir } = await buildPackage(root);
  const native = selectTarget().targetTriple;
  const other = SUPPORTED_TRIPLES.find((triple) => triple !== native);
  assert.throws(
    () => runConsumerSmoke({ packageDir, targetTriple: other, baseDir: root }),
    (error) => error instanceof ConsumerSmokeError && error.code === "TARGET_MISMATCH",
  );
});
