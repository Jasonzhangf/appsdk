import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { gzipSync, gunzipSync } from "node:zlib";

import {
  ArtifactError,
  SKILL_NAMES,
  SUPPORTED_TRIPLES,
  candidateVersion,
  createArchive,
  expectedArchiveName,
  normalizeSourceVersion,
  packageReleaseArtifacts,
  readTarGz,
  stageReleaseArtifacts,
  verifyArchive,
  writeTarGz,
} from "./release-artifacts.js";

const SCRIPT_PATH = fileURLToPath(new URL("./release-artifacts.js", import.meta.url));
const SOURCE_COMMIT = "a".repeat(40);
const SOURCE_VERSION = "9.9.0001";
const NPM_VERSION = "9.9.1";
const CANDIDATE_VERSION = "9.9.1-dev.0";
const PLATFORM_DIRS = Object.freeze({
  "aarch64-apple-darwin": "appsdk-darwin-arm64",
  "x86_64-unknown-linux-gnu": "appsdk-linux-x64-gnu",
  "x86_64-pc-windows-msvc": "appsdk-win32-x64-msvc",
});
const DEFAULT_SKILLS = Object.freeze({
  "appsdk-project-governance/SKILL.md": "---\nname: appsdk-project-governance\n---\n",
  "appsdk-project-governance/references/guide.md": "governance guide\n",
  "appsdk-migration/SKILL.md": "---\nname: appsdk-migration\n---\n",
  "project-memory/SKILL.md": "---\nname: project-memory\n---\n",
});

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function writeFile(path, contents, mode) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
  if (mode !== undefined) {
    chmodSync(path, mode);
  }
}

function binaryNames(targetTriple) {
  const suffix = targetTriple === "x86_64-pc-windows-msvc" ? ".exe" : "";
  return [`appsdk${suffix}`, `project-memory${suffix}`];
}

function platformFiles(targetTriple) {
  const [appsdk, projectMemory] = binaryNames(targetTriple);
  return [
    `bin/${appsdk}`,
    `bin/${projectMemory}`,
    ...SKILL_NAMES.map((name) => `skills/${name}/**`),
  ];
}

function platformRestrictions(targetTriple) {
  if (targetTriple === "aarch64-apple-darwin") {
    return { os: ["darwin"], cpu: ["arm64"] };
  }
  if (targetTriple === "x86_64-unknown-linux-gnu") {
    return { os: ["linux"], cpu: ["x64"], libc: ["glibc"] };
  }
  return { os: ["win32"], cpu: ["x64"] };
}

function writeSkillTree(root, files) {
  for (const [relativePath, contents] of Object.entries(files)) {
    writeFile(join(root, ...relativePath.split("/")), contents);
  }
}

function makeSyntheticRepo(root, skillFiles = DEFAULT_SKILLS, options = {}) {
  const sourceVersion = options.sourceVersion ?? SOURCE_VERSION;
  const packageVersion = options.packageVersion ?? CANDIDATE_VERSION;
  const formal = options.formal ?? false;
  const npmRoot = join(root, "npm");
  const skillRoot = join(root, "sdk-skill-sources");
  const optionalDependencies = Object.fromEntries(
    SUPPORTED_TRIPLES.map((triple) => [
      `@jsonstudio/${PLATFORM_DIRS[triple]}`,
      packageVersion,
    ]),
  );
  writeFile(join(npmRoot, "appsdk", "package.json"), `${JSON.stringify({
    name: "@jsonstudio/appsdk",
    version: packageVersion,
    private: formal ? undefined : true,
    type: "module",
    bin: {
      appsdk: "bin/appsdk.js",
      "project-memory": "bin/project-memory.js",
    },
    files: ["bin/", "lib/", "README.md"],
    engines: { node: ">=24.0.0" },
    optionalDependencies,
    appsdk: {
      sourceVersion,
      npmVersion: normalizeSourceVersion(sourceVersion),
      candidate: formal ? undefined : true,
      artifactStatus: formal ? undefined : "incomplete-until-M5",
    },
  }, null, 2)}\n`);
  writeFile(join(npmRoot, "appsdk", "README.md"), "synthetic main package\n");
  writeFile(join(npmRoot, "appsdk", "bin", "appsdk.js"), "#!/usr/bin/env node\n");
  writeFile(join(npmRoot, "appsdk", "bin", "project-memory.js"), "#!/usr/bin/env node\n");
  writeFile(join(npmRoot, "appsdk", "lib", "launcher.js"), "export {};\n");

  for (const triple of SUPPORTED_TRIPLES) {
    const directory = PLATFORM_DIRS[triple];
    writeFile(join(npmRoot, "platforms", directory, "package.json"), `${JSON.stringify({
      name: `@jsonstudio/${directory}`,
      version: packageVersion,
      private: formal ? undefined : true,
      ...platformRestrictions(triple),
      files: platformFiles(triple),
      appsdk: {
        targetTriple: triple,
        artifactStatus: formal ? undefined : "incomplete-until-M5",
      },
    }, null, 2)}\n`);
    writeFile(join(npmRoot, "platforms", directory, "README.md"), `synthetic ${triple}\n`);
  }
  writeSkillTree(skillRoot, skillFiles);
  return { npmRoot, skillRoot };
}

function skillHashes(skillRoot) {
  const hashes = {};
  for (const skillName of SKILL_NAMES) {
    for (const relativePath of Object.keys(DEFAULT_SKILLS)) {
      if (!relativePath.startsWith(`${skillName}/`)) {
        continue;
      }
      const path = join(skillRoot, ...relativePath.split("/"));
      hashes[relativePath] = sha256(readFileSync(path));
    }
  }
  return Object.fromEntries(Object.entries(hashes).sort(([a], [b]) => a.localeCompare(b)));
}

async function buildArchives(
  root,
  skillRoot,
  { skillRootByTriple = {}, sourceVersion = SOURCE_VERSION } = {},
) {
  const archiveDir = join(root, "archives");
  const binaries = new Map();
  for (const triple of SUPPORTED_TRIPLES) {
    const [appsdkName, projectMemoryName] = binaryNames(triple);
    const binaryDir = join(root, "binaries", triple);
    const appsdkBytes = Buffer.from(`synthetic appsdk ${triple}\n`);
    const projectMemoryBytes = Buffer.from(`synthetic project-memory ${triple}\n`);
    writeFile(join(binaryDir, appsdkName), appsdkBytes, 0o755);
    writeFile(join(binaryDir, projectMemoryName), projectMemoryBytes, 0o755);
    binaries.set(`${triple}/${appsdkName}`, appsdkBytes);
    const stageDir = join(root, "stages", triple);
    await stageReleaseArtifacts({
      targetTriple: triple,
      sourceCommit: SOURCE_COMMIT,
      sourceVersion,
      appsdkBinary: join(binaryDir, appsdkName),
      projectMemoryBinary: join(binaryDir, projectMemoryName),
      skillSourceDir: skillRootByTriple[triple] ?? skillRoot,
      stageDir,
    });
    createArchive({ stageDir, outputDir: archiveDir });
  }
  return { archiveDir, binaries };
}

function tempRoot(t) {
  const root = mkdtempSync(join(tmpdir(), "appsdk-release-artifacts-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test("stage and archive bind file bytes, hashes, and executable mode", async (t) => {
  const root = tempRoot(t);
  const { skillRoot } = makeSyntheticRepo(root);
  const { archiveDir, binaries } = await buildArchives(root, skillRoot);
  const archivePath = join(archiveDir, expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  const { artifact, entries } = verifyArchive(archivePath, {
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: SOURCE_VERSION,
    targetTriple: SUPPORTED_TRIPLES[0],
    expectedSkillHashes: skillHashes(skillRoot),
  });

  assert.equal(artifact.sourceCommit, SOURCE_COMMIT);
  assert.equal(artifact.sourceVersion, SOURCE_VERSION);
  assert.equal(artifact.targetTriple, SUPPORTED_TRIPLES[0]);
  assert.deepEqual(
    [...entries.keys()].sort(),
    ["artifact.json", ...Object.keys(artifact.files)].sort(),
  );
  assert.deepEqual(
    entries.get("bin/appsdk").bytes,
    binaries.get(`${SUPPORTED_TRIPLES[0]}/appsdk`),
  );
  assert.equal(entries.get("bin/appsdk").mode & 0o111, 0o111);
  for (const [path, hash] of Object.entries(artifact.files)) {
    assert.equal(sha256(entries.get(path).bytes), hash);
  }
});

test("archive verification rejects missing and extra files", async (t) => {
  const root = tempRoot(t);
  const { skillRoot } = makeSyntheticRepo(root);
  const { archiveDir } = await buildArchives(root, skillRoot);
  const archivePath = join(archiveDir, expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  const entries = readTarGz(archivePath);
  const options = {
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: SOURCE_VERSION,
    targetTriple: SUPPORTED_TRIPLES[0],
    expectedSkillHashes: skillHashes(skillRoot),
  };

  const missingPath = join(root, "missing", expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  writeTarGz(
    [...entries.entries()]
      .filter(([path]) => path !== "bin/appsdk")
      .map(([path, entry]) => ({ path, ...entry })),
    missingPath,
  );
  assert.throws(
    () => verifyArchive(missingPath, options),
    (error) => error instanceof ArtifactError && error.code === "ARCHIVE_FILE_MISSING",
  );

  const extraPath = join(root, "extra", expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  writeTarGz([
    ...[...entries.entries()].map(([path, entry]) => ({ path, ...entry })),
    { path: "extra.txt", bytes: Buffer.from("unexpected\n"), mode: 0o644 },
  ], extraPath);
  assert.throws(
    () => verifyArchive(extraPath, options),
    (error) => error instanceof ArtifactError && error.code === "ARCHIVE_EXTRA_FILE",
  );
});

test("archive path checks reject absolute and parent traversal entries", async (t) => {
  const root = tempRoot(t);
  for (const path of ["../escape", "/absolute", "a/../../b"]) {
    assert.throws(
      () => writeTarGz([{ path, bytes: Buffer.from("x"), mode: 0o644 }], join(root, "bad.tar.gz")),
      (error) => error instanceof ArtifactError && error.code === "INVALID_TAR_PATH",
    );
  }

  const { skillRoot } = makeSyntheticRepo(root);
  const { archiveDir } = await buildArchives(root, skillRoot);
  const archivePath = join(archiveDir, expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  const mutatedPath = join(root, "mutated.tar.gz");
  const buffer = gunzipSync(readFileSync(archivePath));
  buffer.fill(0, 0, 100);
  Buffer.from("../escape").copy(buffer, 0);
  const header = buffer.subarray(0, 512);
  header.fill(32, 148, 156);
  let checksum = 0;
  for (const byte of header) {
    checksum += byte;
  }
  header.fill(0, 148, 156);
  Buffer.from(checksum.toString(8).padStart(7, "0")).copy(header, 148);
  header[155] = 0;
  writeFileSync(mutatedPath, gzipSync(buffer, { level: 9 }));
  assert.throws(
    () => readTarGz(mutatedPath),
    (error) => error instanceof ArtifactError && error.code === "INVALID_TAR_PATH",
  );
});

test("stage rejects symbolic-link inputs", async (t) => {
  const root = tempRoot(t);
  const { skillRoot } = makeSyntheticRepo(root);
  const binaryDir = join(root, "binaries");
  writeFile(join(binaryDir, "appsdk"), "real\n", 0o755);
  const linkPath = join(binaryDir, "appsdk-link");
  try {
    symlinkSync(join(binaryDir, "appsdk"), linkPath);
  } catch {
    t.skip("symbolic links are not available in this environment");
    return;
  }
  await assert.rejects(
    stageReleaseArtifacts({
      targetTriple: SUPPORTED_TRIPLES[0],
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      appsdkBinary: linkPath,
      projectMemoryBinary: join(binaryDir, "appsdk"),
      skillSourceDir: skillRoot,
      stageDir: join(root, "stage"),
    }),
    (error) => error instanceof ArtifactError && error.code === "BINARY_INPUT_MISSING",
  );
});

test("stage rejects non-executable Unix binaries and extra Skill root entries", async (t) => {
  const root = tempRoot(t);
  const { skillRoot } = makeSyntheticRepo(root);
  const binaryDir = join(root, "binaries");
  writeFile(join(binaryDir, "appsdk"), "not executable\n", 0o644);
  writeFile(join(binaryDir, "project-memory"), "executable\n", 0o755);
  await assert.rejects(
    stageReleaseArtifacts({
      targetTriple: SUPPORTED_TRIPLES[0],
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      appsdkBinary: join(binaryDir, "appsdk"),
      projectMemoryBinary: join(binaryDir, "project-memory"),
      skillSourceDir: skillRoot,
      stageDir: join(root, "stage-nonexec"),
    }),
    (error) => error instanceof ArtifactError && error.code === "BINARY_NOT_EXECUTABLE",
  );

  writeFile(join(skillRoot, "unexpected.txt"), "unexpected\n");
  await assert.rejects(
    stageReleaseArtifacts({
      targetTriple: SUPPORTED_TRIPLES[0],
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      appsdkBinary: join(binaryDir, "project-memory"),
      projectMemoryBinary: join(binaryDir, "project-memory"),
      skillSourceDir: skillRoot,
      stageDir: join(root, "stage-extra-root"),
    }),
    (error) => error instanceof ArtifactError && error.code === "SKILL_SOURCE_INVALID",
  );
});

test("archive verification rejects commit, version, and triple mismatches", async (t) => {
  const root = tempRoot(t);
  const { skillRoot } = makeSyntheticRepo(root);
  const { archiveDir } = await buildArchives(root, skillRoot);
  const archivePath = join(archiveDir, expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]));
  const expectedSkillHashes = skillHashes(skillRoot);

  assert.throws(
    () => verifyArchive(archivePath, {
      sourceCommit: "b".repeat(40),
      sourceVersion: SOURCE_VERSION,
      targetTriple: SUPPORTED_TRIPLES[0],
      expectedSkillHashes,
    }),
    (error) => error instanceof ArtifactError && error.code === "SOURCE_COMMIT_MISMATCH",
  );

  const versionPath = join(root, "version-mismatch", expectedArchiveName("9.9.0002", SUPPORTED_TRIPLES[0]));
  mkdirSync(dirname(versionPath), { recursive: true });
  copyFileSync(archivePath, versionPath);
  assert.throws(
    () => verifyArchive(versionPath, {
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: "9.9.0002",
      targetTriple: SUPPORTED_TRIPLES[0],
      expectedSkillHashes,
    }),
    (error) => error instanceof ArtifactError && error.code === "SOURCE_VERSION_MISMATCH",
  );

  const triplePath = join(root, "triple-mismatch", expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[1]));
  mkdirSync(dirname(triplePath), { recursive: true });
  copyFileSync(archivePath, triplePath);
  assert.throws(
    () => verifyArchive(triplePath, {
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      targetTriple: SUPPORTED_TRIPLES[1],
      expectedSkillHashes,
    }),
    (error) => error instanceof ArtifactError && error.code === "TARGET_TRIPLE_MISMATCH",
  );
});

test("package rejects platform Skill byte drift", async (t) => {
  const root = tempRoot(t);
  const { npmRoot, skillRoot } = makeSyntheticRepo(root);
  const changedSkillRoot = join(root, "changed-skills");
  writeSkillTree(changedSkillRoot, {
    ...DEFAULT_SKILLS,
    "project-memory/SKILL.md": "changed synthetic skill\n",
  });
  const { archiveDir } = await buildArchives(root, skillRoot, {
    skillRootByTriple: { "x86_64-pc-windows-msvc": changedSkillRoot },
  });

  await assert.rejects(
    packageReleaseArtifacts({
      archiveDir,
      outputDir: join(root, "package-output"),
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      skillSourceDir: skillRoot,
      npmRoot,
    }),
    (error) => error instanceof ArtifactError && error.code === "SKILL_HASH_MISMATCH",
  );
});

test("package rejects an archive directory with an unexpected archive", async (t) => {
  const root = tempRoot(t);
  const { npmRoot, skillRoot } = makeSyntheticRepo(root);
  const { archiveDir } = await buildArchives(root, skillRoot);
  writeFile(join(archiveDir, "unexpected.tar.gz"), "not an archive\n");
  await assert.rejects(
    packageReleaseArtifacts({
      archiveDir,
      outputDir: join(root, "package-output"),
      sourceCommit: SOURCE_COMMIT,
      sourceVersion: SOURCE_VERSION,
      skillSourceDir: skillRoot,
      npmRoot,
    }),
    (error) => error instanceof ArtifactError && error.code === "ARCHIVE_SET_MISMATCH",
  );
});

test("package emits four tarballs and sorted SHA256SUMS without changing source metadata", async (t) => {
  const root = tempRoot(t);
  const { npmRoot, skillRoot } = makeSyntheticRepo(root);
  const { archiveDir } = await buildArchives(root, skillRoot);
  const outputDir = join(root, "package-output");
  const result = await packageReleaseArtifacts({
    archiveDir,
    outputDir,
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: SOURCE_VERSION,
    skillSourceDir: skillRoot,
    npmRoot,
  });

  assert.equal(result.tarballs.length, 4);
  const outputEntries = readdirSync(outputDir).sort();
  assert.equal(outputEntries.filter((name) => name.endsWith(".tgz")).length, 4);
  assert.equal(outputEntries.includes("SHA256SUMS"), true);
  for (const name of outputEntries) {
    assert.equal(statSync(join(outputDir, name)).isFile(), true);
    if (name.endsWith(".tgz")) {
      assert.deepEqual(readFileSync(join(outputDir, name)).subarray(0, 2), Buffer.from([0x1f, 0x8b]));
    }
  }

  const sums = readFileSync(result.sha256sums, "utf8").trim().split("\n");
  assert.equal(sums.length, 4);
  const parsed = sums.map((line) => {
    const match = /^([0-9a-f]{64})  (.+\.tgz)$/.exec(line);
    assert.ok(match, `invalid SHA256SUMS line ${JSON.stringify(line)}`);
    return { hash: match[1], name: match[2] };
  });
  assert.deepEqual(parsed.map(({ name }) => name), parsed.map(({ name }) => name).sort());
  for (const { hash, name } of parsed) {
    assert.equal(hash, sha256(readFileSync(join(outputDir, name))));
  }

  const mainTarball = result.tarballs.find((path) => basename(path).startsWith("jsonstudio-appsdk-"));
  assert.ok(mainTarball);
  const mainManifest = JSON.parse(readTarGz(mainTarball).get("package/package.json").bytes.toString("utf8"));
  assert.equal(mainManifest.appsdk.sourceCommit, SOURCE_COMMIT);
  assert.equal(mainManifest.appsdk.sourceVersion, SOURCE_VERSION);

  for (const triple of SUPPORTED_TRIPLES) {
    const manifestPath = join(npmRoot, "platforms", PLATFORM_DIRS[triple], "package.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    assert.equal(manifest.files.includes("artifact.json"), false);
  }
});

test("package binds formal 0.1.15 metadata to the release source commit", async (t) => {
  const releaseSourceVersion = "0.1.0015";
  const releaseNpmVersion = "0.1.15";
  const root = tempRoot(t);
  const { npmRoot, skillRoot } = makeSyntheticRepo(root, DEFAULT_SKILLS, {
    formal: true,
    sourceVersion: releaseSourceVersion,
    packageVersion: releaseNpmVersion,
  });
  const { archiveDir } = await buildArchives(root, skillRoot, {
    sourceVersion: releaseSourceVersion,
  });
  const result = await packageReleaseArtifacts({
    archiveDir,
    outputDir: join(root, "package-output"),
    sourceCommit: SOURCE_COMMIT,
    sourceVersion: releaseSourceVersion,
    skillSourceDir: skillRoot,
    npmRoot,
  });

  assert.equal(result.tarballs.length, 4);
  const mainTarball = result.tarballs.find((path) => basename(path).startsWith("jsonstudio-appsdk-"));
  assert.ok(mainTarball);
  const mainManifest = JSON.parse(readTarGz(mainTarball).get("package/package.json").bytes.toString("utf8"));
  assert.equal(mainManifest.version, releaseNpmVersion);
  assert.equal(mainManifest.private, undefined);
  assert.equal(mainManifest.appsdk.sourceVersion, releaseSourceVersion);
  assert.equal(mainManifest.appsdk.npmVersion, releaseNpmVersion);
  assert.equal(mainManifest.appsdk.sourceCommit, SOURCE_COMMIT);
  assert.equal(mainManifest.appsdk.candidate, undefined);
  assert.equal(mainManifest.appsdk.artifactStatus, undefined);
});

test("version normalization and CLI failures are typed", () => {
  assert.equal(normalizeSourceVersion("9.9.0001"), NPM_VERSION);
  assert.equal(candidateVersion("9.9.0001"), CANDIDATE_VERSION);
  assert.equal(
    expectedArchiveName(SOURCE_VERSION, SUPPORTED_TRIPLES[0]),
    "appsdk-9.9.0001-aarch64-apple-darwin.tar.gz",
  );
  assert.throws(
    () => expectedArchiveName(SOURCE_VERSION, "unsupported"),
    (error) => error instanceof ArtifactError && error.code === "UNSUPPORTED_TARGET_TRIPLE",
  );
  const result = spawnSync(process.execPath, [
    SCRIPT_PATH,
    "stage",
    "--target-triple", "unsupported",
    "--source-commit", SOURCE_COMMIT,
    "--source-version", SOURCE_VERSION,
    "--appsdk-binary", "/missing/appsdk",
    "--project-memory-binary", "/missing/project-memory",
    "--skill-source-dir", "/missing/skills",
    "--stage-dir", join(tmpdir(), "appsdk-release-cli-failure"),
  ], { encoding: "utf8" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /UNSUPPORTED_TARGET_TRIPLE/);
});

test("module import without process.argv[1] does not enter the CLI guard", () => {
  const moduleUrl = new URL("./release-artifacts.js", import.meta.url).href;
  const result = spawnSync(process.execPath, [
    "--input-type=module",
    "-e",
    `await import(${JSON.stringify(moduleUrl)}); process.stdout.write("import-ok\\n");`,
  ], { encoding: "utf8" });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "import-ok\n");
});
