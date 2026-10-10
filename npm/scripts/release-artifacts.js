#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync, gunzipSync } from "node:zlib";
import { isDeepStrictEqual } from "node:util";

import { mapSourceVersion } from "../appsdk/lib/version.js";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));
const TAR_BLOCK = 512;
const TAR_HEADER_SIZE = 512;
const EXECUTABLE_MODE = 0o755;
const REGULAR_FILE_TYPE = "0";
const COMMIT_PATTERN = /^[0-9a-f]{40}$/;

export const SKILL_NAMES = Object.freeze([
  "appsdk-project-governance",
  "appsdk-migration",
  "project-memory",
]);
export const SUPPORTED_TRIPLES = Object.freeze([
  "aarch64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]);

const PLATFORM_PACKAGES = Object.freeze({
  "aarch64-apple-darwin": "appsdk-darwin-arm64",
  "x86_64-unknown-linux-gnu": "appsdk-linux-x64-gnu",
  "x86_64-pc-windows-msvc": "appsdk-win32-x64-msvc",
});
const PLATFORM_RESTRICTIONS = Object.freeze({
  "aarch64-apple-darwin": Object.freeze({ os: ["darwin"], cpu: ["arm64"] }),
  "x86_64-unknown-linux-gnu": Object.freeze({ os: ["linux"], cpu: ["x64"], libc: ["glibc"] }),
  "x86_64-pc-windows-msvc": Object.freeze({ os: ["win32"], cpu: ["x64"] }),
});

export class ArtifactError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "ArtifactError";
    this.code = code;
  }
}

export function normalizeSourceVersion(sourceVersion) {
  try {
    return mapSourceVersion(sourceVersion);
  } catch (error) {
    throw new ArtifactError(
      "INVALID_SOURCE_VERSION",
      `Invalid source version ${JSON.stringify(sourceVersion)}; expected MAJOR.MINOR.PPPP.`,
      { cause: error },
    );
  }
}

export function candidateVersion(sourceVersion) {
  return `${normalizeSourceVersion(sourceVersion)}-dev.0`;
}

function assertSupportedTriple(targetTriple) {
  if (!SUPPORTED_TRIPLES.includes(targetTriple)) {
    throw new ArtifactError(
      "UNSUPPORTED_TARGET_TRIPLE",
      `Unsupported target triple ${JSON.stringify(targetTriple)}; expected one of ${SUPPORTED_TRIPLES.join(", ")}.`,
    );
  }
}

function assertSourceCommit(sourceCommit) {
  if (!COMMIT_PATTERN.test(sourceCommit ?? "")) {
    throw new ArtifactError(
      "INVALID_SOURCE_COMMIT",
      `Invalid source commit ${JSON.stringify(sourceCommit)}; expected a full 40-character lowercase SHA-1.`,
    );
  }
}

function assertSha256(value, code = "INVALID_SHA256") {
  if (!/^[0-9a-f]{64}$/.test(value)) {
    throw new ArtifactError(
      code,
      `Invalid SHA-256 ${JSON.stringify(value)}; expected 64 lowercase hexadecimal characters.`,
    );
  }
}

function isSafeRelativePath(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 100
    && !value.includes("\\")
    && !value.includes("\0")
    && !value.startsWith("/")
    && value !== "."
    && value !== ".."
    && value.split("/").every((part) => part && part !== "." && part !== "..");
}

function toPosixPath(value) {
  return value.split(sep).join("/");
}

function hashBytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function hashFile(filePath) {
  return hashBytes(readFileSync(filePath));
}

function walkFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const entryPath = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...walkFiles(entryPath));
    } else if (entry.isFile()) {
      files.push(entryPath);
    } else {
      throw new ArtifactError(
        "INVALID_STAGE_ENTRY",
        `Stage entry ${entryPath} is not a regular file; symlinks and special files are not allowed.`,
      );
    }
  }
  return files;
}

function binaryNames(targetTriple) {
  const suffix = targetTriple === "x86_64-pc-windows-msvc" ? ".exe" : "";
  return [`bin/appsdk${suffix}`, `bin/project-memory${suffix}`];
}

function expectedStageRoots(targetTriple) {
  return [
    ...binaryNames(targetTriple),
    ...SKILL_NAMES.map((name) => `skills/${name}`),
  ];
}

function listStageFiles(directory, targetTriple) {
  const allowedRoots = expectedStageRoots(targetTriple);
  const files = new Map();
  for (const filePath of walkFiles(directory)) {
    const path = toPosixPath(relative(directory, filePath));
    if (path === "artifact.json") {
      continue;
    }
    if (!isSafeRelativePath(path)) {
      throw new ArtifactError("INVALID_STAGE_PATH", `Invalid stage path ${JSON.stringify(path)}.`);
    }
    if (!allowedRoots.some((root) => path === root || path.startsWith(`${root}/`))) {
      throw new ArtifactError(
        "EXTRA_STAGE_FILE",
        `Unexpected stage file ${JSON.stringify(path)}; allowed roots are ${allowedRoots.join(", ")}.`,
      );
    }
    files.set(path, filePath);
  }
  return files;
}

function assertEmptyDirectory(directory) {
  try {
    mkdirSync(directory, { recursive: true });
  } catch (error) {
    throw new ArtifactError("OUTPUT_DIRECTORY_INVALID", `Could not create output directory ${directory}.`, { cause: error });
  }
  if (!statSync(directory).isDirectory()) {
    throw new ArtifactError("OUTPUT_DIRECTORY_INVALID", `Output path ${directory} is not a directory.`);
  }
  if (readdirSync(directory).length !== 0) {
    throw new ArtifactError("OUTPUT_DIRECTORY_NOT_EMPTY", `Output directory ${directory} is not empty.`);
  }
}

function assertDeepEqual(actual, expected, code, message) {
  if (!isDeepStrictEqual(actual, expected)) {
    throw new ArtifactError(code, `${message} Expected ${JSON.stringify(expected)}, found ${JSON.stringify(actual)}.`);
  }
}

function assertSkillTrees(skillSourceDir) {
  let rootInfo;
  try {
    rootInfo = lstatSync(skillSourceDir);
  } catch (error) {
    throw new ArtifactError("SKILL_SOURCE_MISSING", `Skill source root ${skillSourceDir} is missing.`, { cause: error });
  }
  if (!rootInfo.isDirectory() || rootInfo.isSymbolicLink()) {
    throw new ArtifactError("SKILL_SOURCE_INVALID", `Skill source root ${skillSourceDir} is not a directory.`);
  }
  const rootEntries = readdirSync(skillSourceDir, { withFileTypes: true });
  const expectedNames = [...SKILL_NAMES].sort();
  if (
    rootEntries.length !== expectedNames.length
    || rootEntries.some((entry) => !entry.isDirectory() || !expectedNames.includes(entry.name))
  ) {
    throw new ArtifactError(
      "SKILL_SOURCE_INVALID",
      `Skill source root ${skillSourceDir} must contain exactly ${expectedNames.join(", ")}.`,
    );
  }
  for (const skillName of SKILL_NAMES) {
    const skillDir = join(skillSourceDir, skillName);
    let info;
    try {
      info = lstatSync(skillDir);
    } catch (error) {
      throw new ArtifactError("SKILL_SOURCE_MISSING", `Skill source directory ${skillDir} is missing.`, { cause: error });
    }
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new ArtifactError("SKILL_SOURCE_INVALID", `Skill source ${skillDir} is not a directory.`);
    }
    if (!walkFiles(skillDir).some((path) => basename(path) === "SKILL.md")) {
      throw new ArtifactError("SKILL_SOURCE_INVALID", `Skill source ${skillDir} has no SKILL.md.`);
    }
  }
}

export function expectedArchiveName(sourceVersion, targetTriple) {
  assertSupportedTriple(targetTriple);
  normalizeSourceVersion(sourceVersion);
  return `appsdk-${sourceVersion}-${targetTriple}.tar.gz`;
}

function assertRegularFile(filePath, code, label) {
  let info;
  try {
    info = lstatSync(filePath);
  } catch (error) {
    throw new ArtifactError(code, `${label} ${filePath} is missing.`, { cause: error });
  }
  if (!info.isFile() || info.isSymbolicLink()) {
    throw new ArtifactError(code, `${label} ${filePath} is not a regular file.`);
  }
  return info;
}

export async function stageReleaseArtifacts({
  targetTriple,
  sourceCommit,
  sourceVersion,
  appsdkBinary,
  projectMemoryBinary,
  skillSourceDir,
  stageDir,
}) {
  assertSupportedTriple(targetTriple);
  assertSourceCommit(sourceCommit);
  normalizeSourceVersion(sourceVersion);
  const appsdkInfo = assertRegularFile(appsdkBinary, "BINARY_INPUT_MISSING", "AppSDK binary");
  const projectMemoryInfo = assertRegularFile(projectMemoryBinary, "BINARY_INPUT_MISSING", "project-memory binary");
  if (targetTriple !== "x86_64-pc-windows-msvc") {
    for (const [label, info] of [["AppSDK binary", appsdkInfo], ["project-memory binary", projectMemoryInfo]]) {
      if ((info.mode & 0o111) !== 0o111) {
        throw new ArtifactError("BINARY_NOT_EXECUTABLE", `${label} must be executable.`);
      }
    }
  }
  assertSkillTrees(skillSourceDir);
  assertEmptyDirectory(stageDir);

  const [appsdkStagePath, projectMemoryStagePath] = binaryNames(targetTriple);
  const files = {};
  for (const [stagePath, sourcePath] of [
    [appsdkStagePath, appsdkBinary],
    [projectMemoryStagePath, projectMemoryBinary],
  ]) {
    const destination = join(stageDir, ...stagePath.split("/"));
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(sourcePath, destination);
    chmodSync(destination, EXECUTABLE_MODE);
    files[stagePath] = hashFile(destination);
  }

  for (const skillName of SKILL_NAMES) {
    const skillDir = join(skillSourceDir, skillName);
    for (const filePath of walkFiles(skillDir)) {
      const relativePath = toPosixPath(relative(skillDir, filePath));
      const stagePath = `skills/${skillName}/${relativePath}`;
      if (!isSafeRelativePath(stagePath)) {
        throw new ArtifactError("INVALID_SKILL_PATH", `Invalid Skill file path ${JSON.stringify(stagePath)}.`);
      }
      const destination = join(stageDir, ...stagePath.split("/"));
      mkdirSync(dirname(destination), { recursive: true });
      copyFileSync(filePath, destination);
      files[stagePath] = hashFile(destination);
    }
  }

  const artifact = {
    sourceCommit,
    sourceVersion,
    targetTriple,
    files: Object.fromEntries(Object.entries(files).sort(([a], [b]) => a.localeCompare(b))),
  };
  writeFileSync(join(stageDir, "artifact.json"), `${JSON.stringify(artifact, null, 2)}\n`);
  return artifact;
}

function tarString(value, length) {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length > length) {
    throw new ArtifactError("TAR_METADATA_TOO_LONG", `Tar metadata ${JSON.stringify(value)} is too long.`);
  }
  const result = Buffer.alloc(length);
  result.set(bytes);
  return result;
}

function tarOctal(value, length) {
  return tarString(value.toString(8).padStart(length - 1, "0"), length);
}

function tarHeader({ path, size, mode, mtime = 0 }) {
  if (!isSafeRelativePath(path)) {
    throw new ArtifactError("INVALID_TAR_PATH", `Invalid tar entry path ${JSON.stringify(path)}.`);
  }
  const header = Buffer.alloc(TAR_HEADER_SIZE);
  tarString(path, 100).copy(header, 0);
  tarOctal(mode, 8).copy(header, 100);
  tarOctal(0, 8).copy(header, 108);
  tarOctal(0, 8).copy(header, 116);
  tarOctal(size, 12).copy(header, 124);
  tarOctal(mtime, 12).copy(header, 136);
  header.fill(32, 148, 156);
  tarString(REGULAR_FILE_TYPE, 1).copy(header, 156);
  tarString("ustar", 6).copy(header, 257);
  tarString("00", 2).copy(header, 263);
  tarOctal(checksumForHeader(header), 8).copy(header, 148);
  return header;
}

function checksumForHeader(header) {
  const copy = Buffer.from(header);
  copy.fill(32, 148, 156);
  let checksum = 0;
  for (const byte of copy) {
    checksum += byte;
  }
  return checksum;
}

export function writeTarGz(entries, outputPath) {
  mkdirSync(dirname(outputPath), { recursive: true });
  const chunks = [];
  for (const { path, bytes, mode } of [...entries].sort((a, b) => a.path.localeCompare(b.path))) {
    if (!Buffer.isBuffer(bytes)) {
      throw new ArtifactError("INVALID_TAR_ENTRY", `Tar entry ${path} must carry a Buffer.`);
    }
    chunks.push(tarHeader({ path, size: bytes.length, mode: mode ?? 0o644 }));
    chunks.push(bytes);
    const padding = (TAR_BLOCK - (bytes.length % TAR_BLOCK)) % TAR_BLOCK;
    if (padding) {
      chunks.push(Buffer.alloc(padding));
    }
  }
  chunks.push(Buffer.alloc(TAR_BLOCK * 2));
  writeFileSync(outputPath, gzipSync(Buffer.concat(chunks), { level: 9 }));
  return hashFile(outputPath);
}

export function readTarGz(archivePath) {
  let compressed;
  try {
    compressed = readFileSync(archivePath);
  } catch (error) {
    throw new ArtifactError("ARCHIVE_MISSING", `Archive ${archivePath} is missing.`, { cause: error });
  }
  let buffer;
  try {
    buffer = gunzipSync(compressed);
  } catch (error) {
    throw new ArtifactError("ARCHIVE_INVALID", `Archive ${archivePath} is not a valid gzip stream.`, { cause: error });
  }
  const entries = new Map();
  let offset = 0;
  while (offset + TAR_HEADER_SIZE <= buffer.length) {
    const header = buffer.subarray(offset, offset + TAR_HEADER_SIZE);
    if (header.every((byte) => byte === 0)) {
      break;
    }
    const decode = (start, length) => {
      const raw = header.subarray(start, start + length);
      const end = raw.indexOf(0);
      return raw.subarray(0, end === -1 ? raw.length : end).toString("utf8");
    };
    const path = decode(0, 100);
    const mode = Number.parseInt(decode(100, 8), 8);
    const size = Number.parseInt(decode(124, 12), 8);
    const type = decode(156, 1) || REGULAR_FILE_TYPE;
    const checksum = Number.parseInt(decode(148, 8), 8);
    if (!Number.isSafeInteger(size) || size < 0) {
      throw new ArtifactError("INVALID_TAR_ENTRY", `Invalid size for tar entry ${JSON.stringify(path)}.`);
    }
    if (checksum !== checksumForHeader(header)) {
      throw new ArtifactError("INVALID_TAR_CHECKSUM", `Invalid checksum for tar entry ${JSON.stringify(path)}.`);
    }
    if (type !== REGULAR_FILE_TYPE) {
      throw new ArtifactError("INVALID_TAR_ENTRY", `Tar entry ${JSON.stringify(path)} is not a regular file.`);
    }
    if (!isSafeRelativePath(path)) {
      throw new ArtifactError("INVALID_TAR_PATH", `Invalid tar entry path ${JSON.stringify(path)}.`);
    }
    if (entries.has(path)) {
      throw new ArtifactError("DUPLICATE_TAR_ENTRY", `Duplicate tar entry ${JSON.stringify(path)}.`);
    }
    const start = offset + TAR_HEADER_SIZE;
    const end = start + size;
    if (end > buffer.length) {
      throw new ArtifactError("INVALID_TAR_ENTRY", `Truncated tar entry ${JSON.stringify(path)}.`);
    }
    entries.set(path, { bytes: Buffer.from(buffer.subarray(start, end)), mode: mode & 0o777 });
    offset = start + Math.ceil(size / TAR_BLOCK) * TAR_BLOCK;
  }
  if (entries.size === 0) {
    throw new ArtifactError("EMPTY_ARCHIVE", `Archive ${archivePath} has no file entries.`);
  }
  return entries;
}

export function createArchive({ stageDir, outputDir }) {
  const artifact = readJson(join(stageDir, "artifact.json"), "ARTIFACT_MANIFEST_INVALID");
  assertSupportedTriple(artifact.targetTriple);
  normalizeSourceVersion(artifact.sourceVersion);
  assertSourceCommit(artifact.sourceCommit);
  const stageFiles = listStageFiles(stageDir, artifact.targetTriple);
  verifyArtifactManifest(artifact, stageFiles, {
    sourceCommit: artifact.sourceCommit,
    sourceVersion: artifact.sourceVersion,
    targetTriple: artifact.targetTriple,
    skillPaths: Object.keys(artifact.files).filter((path) => path.startsWith("skills/")),
  });
  mkdirSync(outputDir, { recursive: true });
  const archivePath = join(outputDir, expectedArchiveName(artifact.sourceVersion, artifact.targetTriple));
  const entries = [{
    path: "artifact.json",
    bytes: readFileSync(join(stageDir, "artifact.json")),
    mode: 0o644,
  }];
  for (const [path, filePath] of stageFiles) {
    entries.push({
      path,
      bytes: readFileSync(filePath),
      mode: path.startsWith("bin/") ? EXECUTABLE_MODE : statSync(filePath).mode & 0o777,
    });
  }
  writeTarGz(entries, archivePath);
  return archivePath;
}

function readJson(filePath, code) {
  try {
    return JSON.parse(readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new ArtifactError(code, `Could not read JSON ${filePath}: ${error.message}`, { cause: error });
  }
}

function verifyArtifactManifest(artifact, stageFiles, options) {
  const expectedKeys = ["files", "sourceCommit", "sourceVersion", "targetTriple"];
  if (JSON.stringify(Object.keys(artifact).sort()) !== JSON.stringify(expectedKeys)) {
    throw new ArtifactError("ARTIFACT_MANIFEST_INVALID", "artifact.json must contain only sourceCommit, sourceVersion, targetTriple, and files.");
  }
  if (artifact.sourceCommit !== options.sourceCommit) {
    throw new ArtifactError("SOURCE_COMMIT_MISMATCH", `Source commit mismatch: expected ${options.sourceCommit}, found ${String(artifact.sourceCommit)}.`);
  }
  if (artifact.sourceVersion !== options.sourceVersion) {
    throw new ArtifactError("SOURCE_VERSION_MISMATCH", `Source version mismatch: expected ${options.sourceVersion}, found ${String(artifact.sourceVersion)}.`);
  }
  if (artifact.targetTriple !== options.targetTriple) {
    throw new ArtifactError("TARGET_TRIPLE_MISMATCH", `Target triple mismatch: expected ${options.targetTriple}, found ${String(artifact.targetTriple)}.`);
  }
  if (!artifact.files || typeof artifact.files !== "object" || Array.isArray(artifact.files)) {
    throw new ArtifactError("ARTIFACT_MANIFEST_INVALID", "artifact.files must be an object.");
  }
  const expectedFiles = [
    ...binaryNames(artifact.targetTriple),
    ...options.skillPaths,
  ];
  const actualFiles = Object.keys(artifact.files);
  if (actualFiles.length !== expectedFiles.length || new Set(actualFiles).size !== expectedFiles.length) {
    throw new ArtifactError("ARTIFACT_FILE_SET_MISMATCH", "artifact.files has the wrong file count or duplicate paths.");
  }
  for (const path of expectedFiles) {
    if (!(path in artifact.files)) {
      throw new ArtifactError("ARTIFACT_FILE_MISSING", `artifact.files is missing ${JSON.stringify(path)}.`);
    }
  }
  for (const [path, hash] of Object.entries(artifact.files)) {
    if (!isSafeRelativePath(path)) {
      throw new ArtifactError("INVALID_ARTIFACT_PATH", `artifact.files contains invalid path ${JSON.stringify(path)}.`);
    }
    assertSha256(hash, "INVALID_ARTIFACT_HASH");
    if (stageFiles && !stageFiles.has(path)) {
      throw new ArtifactError("ARTIFACT_FILE_MISSING", `Stage is missing ${JSON.stringify(path)}.`);
    }
    if (stageFiles && hashFile(stageFiles.get(path)) !== hash) {
      throw new ArtifactError("ARTIFACT_HASH_MISMATCH", `Stage file ${JSON.stringify(path)} does not match its manifest hash.`);
    }
  }
  if (stageFiles) {
    for (const path of stageFiles.keys()) {
      if (!(path in artifact.files)) {
        throw new ArtifactError("EXTRA_STAGE_FILE", `Stage contains unexpected file ${JSON.stringify(path)}.`);
      }
    }
  }
}

export function verifyArchive(archivePath, {
  sourceCommit,
  sourceVersion,
  targetTriple,
  expectedSkillHashes,
}) {
  const expectedName = expectedArchiveName(sourceVersion, targetTriple);
  if (basename(archivePath) !== expectedName) {
    throw new ArtifactError(
      "ARCHIVE_NAME_MISMATCH",
      `Archive name ${JSON.stringify(basename(archivePath))} does not match expected ${JSON.stringify(expectedName)}.`,
    );
  }
  const entries = readTarGz(archivePath);
  if (!entries.has("artifact.json")) {
    throw new ArtifactError("ARTIFACT_MANIFEST_MISSING", `Archive ${archivePath} has no artifact.json.`);
  }
  let artifact;
  try {
    artifact = JSON.parse(entries.get("artifact.json").bytes.toString("utf8"));
  } catch (error) {
    throw new ArtifactError(
      "ARTIFACT_MANIFEST_INVALID",
      `Could not parse artifact.json in ${archivePath}: ${error.message}`,
      { cause: error },
    );
  }
  const skillPaths = Object.keys(expectedSkillHashes ?? {})
    .map((path) => `skills/${path}`)
    .sort();
  verifyArtifactManifest(artifact, null, {
    sourceCommit,
    sourceVersion,
    targetTriple,
    skillPaths,
  });
  for (const [relativePath, expectedHash] of Object.entries(expectedSkillHashes ?? {})) {
    const path = `skills/${relativePath}`;
    if (artifact.files[path] !== expectedHash) {
      throw new ArtifactError(
        "SKILL_HASH_MISMATCH",
        `Skill ${JSON.stringify(relativePath)} in ${archivePath} does not match the source tree.`,
      );
    }
  }

  const expectedFiles = new Set(["artifact.json", ...Object.keys(artifact.files)]);
  const actualFiles = new Set(entries.keys());
  for (const path of expectedFiles) {
    if (!actualFiles.has(path)) {
      throw new ArtifactError("ARCHIVE_FILE_MISSING", `Archive ${archivePath} is missing ${JSON.stringify(path)}.`);
    }
  }
  for (const path of actualFiles) {
    if (!expectedFiles.has(path)) {
      throw new ArtifactError("ARCHIVE_EXTRA_FILE", `Archive ${archivePath} contains unexpected file ${JSON.stringify(path)}.`);
    }
  }

  for (const [path, expectedHash] of Object.entries(artifact.files)) {
    const entry = entries.get(path);
    const actualHash = hashBytes(entry.bytes);
    if (actualHash !== expectedHash) {
      throw new ArtifactError(
        "ARCHIVE_HASH_MISMATCH",
        `File ${JSON.stringify(path)} in ${archivePath} has SHA-256 ${actualHash}, expected ${expectedHash}.`,
      );
    }
    if (path.startsWith("bin/") && (entry.mode & 0o111) !== 0o111) {
      throw new ArtifactError(
        "BINARY_NOT_EXECUTABLE",
        `Binary ${JSON.stringify(path)} in ${archivePath} does not preserve executable mode.`,
      );
    }
  }
  return { artifact, entries };
}

function expectedPlatformFiles(targetTriple) {
  return [
    ...binaryNames(targetTriple),
    ...SKILL_NAMES.map((name) => `skills/${name}/**`),
  ];
}

function validateNpmMetadata({ mainPackage, platformPackage, sourceVersion, targetTriple }) {
  const npmVersion = normalizeSourceVersion(sourceVersion);
  const candidate = candidateVersion(sourceVersion);
  if (mainPackage.name !== "@jsonstudio/appsdk") {
    throw new ArtifactError("NPM_PACKAGE_NAME_MISMATCH", "Main package name must be @jsonstudio/appsdk.");
  }
  if (mainPackage.version !== npmVersion && mainPackage.version !== candidate) {
    throw new ArtifactError(
      "NPM_VERSION_MISMATCH",
      `Main package version ${JSON.stringify(mainPackage.version)} must map from ${sourceVersion}.`,
    );
  }
  if (mainPackage.appsdk?.sourceVersion !== sourceVersion || mainPackage.appsdk?.npmVersion !== npmVersion) {
    throw new ArtifactError("NPM_SOURCE_VERSION_MISMATCH", "Main package source/npm version mapping does not match the release input.");
  }
  const expectedOptional = Object.fromEntries(
    SUPPORTED_TRIPLES.map((triple) => [`@jsonstudio/${PLATFORM_PACKAGES[triple]}`, mainPackage.version]),
  );
  assertDeepEqual(
    mainPackage.optionalDependencies,
    expectedOptional,
    "NPM_OPTIONAL_DEPENDENCIES_MISMATCH",
    "Main package optionalDependencies must pin the three exact platform packages.",
  );

  const expectedName = `@jsonstudio/${PLATFORM_PACKAGES[targetTriple]}`;
  if (platformPackage.name !== expectedName) {
    throw new ArtifactError("NPM_PACKAGE_NAME_MISMATCH", `Platform package must be ${expectedName}.`);
  }
  if (platformPackage.version !== mainPackage.version) {
    throw new ArtifactError("NPM_VERSION_MISMATCH", `Platform package ${expectedName} version must equal ${mainPackage.version}.`);
  }
  if (platformPackage.appsdk?.targetTriple !== targetTriple) {
    throw new ArtifactError("NPM_TARGET_MISMATCH", `Platform package target must be ${targetTriple}.`);
  }
  const actualRestrictions = {
    os: platformPackage.os,
    cpu: platformPackage.cpu,
    ...(platformPackage.libc === undefined ? {} : { libc: platformPackage.libc }),
  };
  assertDeepEqual(
    actualRestrictions,
    PLATFORM_RESTRICTIONS[targetTriple],
    "NPM_TARGET_RESTRICTIONS_MISMATCH",
    `Platform package ${expectedName} must restrict os/cpu/libc exactly.`,
  );
  assertDeepEqual(
    platformPackage.files,
    expectedPlatformFiles(targetTriple),
    "NPM_FILES_MISMATCH",
    `Platform package ${expectedName} must declare the exact runtime file allowlist.`,
  );
}

async function sourceSkillHashes(skillSourceDir) {
  const hashes = {};
  for (const skillName of SKILL_NAMES) {
    const skillDir = join(skillSourceDir, skillName);
    for (const filePath of walkFiles(skillDir)) {
      const relativePath = toPosixPath(relative(skillDir, filePath));
      hashes[`${skillName}/${relativePath}`] = hashFile(filePath);
    }
  }
  return Object.fromEntries(Object.entries(hashes).sort(([a], [b]) => a.localeCompare(b)));
}

function copyPackageFiles(sourceDir, destinationDir) {
  for (const filePath of walkFiles(sourceDir)) {
    const relativePath = toPosixPath(relative(sourceDir, filePath));
    const destination = join(destinationDir, ...relativePath.split("/"));
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(filePath, destination);
  }
}

export function runNpmPack({ packageDir, outputDir, cacheDir }) {
  const npmExecPath = process.env.npm_execpath;
  const command = npmExecPath ?? (process.platform === "win32" ? "npm.cmd" : "npm");
  const npmArgs = [
    "--cache", cacheDir,
    "pack",
    "--pack-destination", outputDir,
    "--ignore-scripts",
    "--json",
  ];
  let stdout;
  try {
    stdout = execFileSync(command, npmExecPath ? [npmExecPath, ...npmArgs] : npmArgs, {
      cwd: packageDir,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
      env: {
        ...process.env,
        npm_config_offline: "true",
        npm_config_audit: "false",
        npm_config_fund: "false",
      },
    });
  } catch (error) {
    throw new ArtifactError("NPM_PACK_FAILED", `npm pack failed for ${packageDir}: ${error.message}`, { cause: error });
  }
  let result;
  try {
    result = JSON.parse(stdout);
  } catch (error) {
    throw new ArtifactError("NPM_PACK_OUTPUT_INVALID", `npm pack did not return JSON for ${packageDir}.`, { cause: error });
  }
  const fileName = result?.[0]?.filename;
  if (!fileName) {
    throw new ArtifactError("NPM_PACK_OUTPUT_INVALID", `npm pack did not report a tarball for ${packageDir}.`);
  }
  const outputPath = join(outputDir, basename(fileName));
  if (!statSync(outputPath).isFile()) {
    throw new ArtifactError("NPM_PACK_OUTPUT_MISSING", `npm pack output ${outputPath} is missing.`);
  }
  return outputPath;
}

export async function packageReleaseArtifacts({
  archiveDir,
  outputDir,
  sourceCommit,
  sourceVersion,
  skillSourceDir,
  npmRoot = join(ROOT, "npm"),
  cacheDir,
  runPack = runNpmPack,
}) {
  assertSourceCommit(sourceCommit);
  normalizeSourceVersion(sourceVersion);
  assertSkillTrees(skillSourceDir);
  assertEmptyDirectory(outputDir);
  const expectedArchives = new Set(
    SUPPORTED_TRIPLES.map((triple) => expectedArchiveName(sourceVersion, triple)),
  );
  let archiveEntries;
  try {
    archiveEntries = readdirSync(archiveDir, { withFileTypes: true });
  } catch (error) {
    throw new ArtifactError("ARCHIVE_DIRECTORY_MISSING", `Archive directory ${archiveDir} is missing.`, { cause: error });
  }
  for (const entry of archiveEntries) {
    if (!entry.isFile() || !expectedArchives.has(entry.name)) {
      throw new ArtifactError(
        "ARCHIVE_SET_MISMATCH",
        `Archive directory ${archiveDir} must contain exactly ${[...expectedArchives].join(", ")}.`,
      );
    }
  }
  const expectedSkillHashes = await sourceSkillHashes(skillSourceDir);
  const tempRoot = mkdtempSync(join(tmpdir(), "appsdk-release-pack-"));
  const packCacheDir = cacheDir ?? join(tempRoot, "npm-cache");
  mkdirSync(packCacheDir, { recursive: true });
  const tarballs = [];
  try {
    const mainPackage = readJson(join(npmRoot, "appsdk", "package.json"), "NPM_MANIFEST_INVALID");
    for (const targetTriple of SUPPORTED_TRIPLES) {
      const archivePath = join(archiveDir, expectedArchiveName(sourceVersion, targetTriple));
      const { entries } = verifyArchive(archivePath, {
        sourceCommit,
        sourceVersion,
        targetTriple,
        expectedSkillHashes,
      });
      const platformDirName = PLATFORM_PACKAGES[targetTriple];
      const sourcePackageDir = join(npmRoot, "platforms", platformDirName);
      const platformPackage = readJson(join(sourcePackageDir, "package.json"), "NPM_MANIFEST_INVALID");
      validateNpmMetadata({ mainPackage, platformPackage, sourceVersion, targetTriple });
      const tempPackageDir = join(tempRoot, platformDirName);
      copyPackageFiles(sourcePackageDir, tempPackageDir);
      for (const [path, entry] of entries) {
        const destination = join(tempPackageDir, ...path.split("/"));
        mkdirSync(dirname(destination), { recursive: true });
        writeFileSync(destination, entry.bytes);
        chmodSync(destination, path.startsWith("bin/") ? EXECUTABLE_MODE : entry.mode);
      }
      writeFileSync(join(tempPackageDir, "package.json"), `${JSON.stringify({
        ...platformPackage,
        files: [...platformPackage.files, "artifact.json"],
      }, null, 2)}\n`);
      tarballs.push(runPack({ packageDir: tempPackageDir, outputDir, cacheDir: packCacheDir }));
    }

    const tempMainDir = join(tempRoot, "appsdk");
    copyPackageFiles(join(npmRoot, "appsdk"), tempMainDir);
    const tempMainManifestPath = join(tempMainDir, "package.json");
    const tempMainManifest = readJson(tempMainManifestPath, "NPM_MANIFEST_INVALID");
    tempMainManifest.appsdk = {
      ...tempMainManifest.appsdk,
      sourceVersion,
      sourceCommit,
    };
    writeFileSync(tempMainManifestPath, `${JSON.stringify(tempMainManifest, null, 2)}\n`);
    tarballs.push(runPack({ packageDir: tempMainDir, outputDir, cacheDir: packCacheDir }));

    const expectedTarballCount = 4;
    if (tarballs.length !== expectedTarballCount) {
      throw new ArtifactError("NPM_PACK_COUNT_MISMATCH", `Expected ${expectedTarballCount} tarballs, found ${tarballs.length}.`);
    }
    const sortedTarballs = [...tarballs].sort((a, b) => basename(a).localeCompare(basename(b)));
    const sums = sortedTarballs.map((path) => `${hashFile(path)}  ${basename(path)}`);
    writeFileSync(join(outputDir, "SHA256SUMS"), `${sums.join("\n")}\n`);
    return {
      tarballs: sortedTarballs,
      sha256sums: join(outputDir, "SHA256SUMS"),
      skillHashes: expectedSkillHashes,
    };
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
}

function parseFlagArgs(argv, allowedFlags) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowedFlags.includes(flag) || value === undefined || options[flag] !== undefined) {
      throw new ArtifactError("INVALID_ARGUMENTS", `Invalid arguments ${JSON.stringify(argv)}; expected ${allowedFlags.join(" ")}.`);
    }
    options[flag] = value;
  }
  return options;
}

function requireFlags(options, flags) {
  for (const flag of flags) {
    if (options[flag] === undefined) {
      throw new ArtifactError("INVALID_ARGUMENTS", `Missing required argument ${flag}.`);
    }
  }
}

export function parseArgs(argv) {
  const [command, ...rest] = argv;
  if (command === "stage") {
    const flags = [
      "--target-triple", "--source-commit", "--source-version",
      "--appsdk-binary", "--project-memory-binary", "--skill-source-dir", "--stage-dir",
    ];
    const options = parseFlagArgs(rest, flags);
    requireFlags(options, flags);
    return {
      command,
      options: {
        targetTriple: options["--target-triple"],
        sourceCommit: options["--source-commit"],
        sourceVersion: options["--source-version"],
        appsdkBinary: resolve(options["--appsdk-binary"]),
        projectMemoryBinary: resolve(options["--project-memory-binary"]),
        skillSourceDir: resolve(options["--skill-source-dir"]),
        stageDir: resolve(options["--stage-dir"]),
      },
    };
  }
  if (command === "archive") {
    const flags = ["--stage-dir", "--output-dir"];
    const options = parseFlagArgs(rest, flags);
    requireFlags(options, flags);
    return {
      command,
      options: {
        stageDir: resolve(options["--stage-dir"]),
        outputDir: resolve(options["--output-dir"]),
      },
    };
  }
  if (command === "package") {
    const flags = ["--archive-dir", "--output-dir", "--source-commit", "--source-version", "--skill-source-dir", "--npm-root", "--cache-dir"];
    const options = parseFlagArgs(rest, flags);
    requireFlags(options, flags.slice(0, 4));
    return {
      command,
      options: {
        archiveDir: resolve(options["--archive-dir"]),
        outputDir: resolve(options["--output-dir"]),
        sourceCommit: options["--source-commit"],
        sourceVersion: options["--source-version"],
        skillSourceDir: options["--skill-source-dir"]
          ? resolve(options["--skill-source-dir"])
          : join(ROOT, "sdk-skill-sources"),
        ...(options["--npm-root"] ? { npmRoot: resolve(options["--npm-root"]) } : {}),
        ...(options["--cache-dir"] ? { cacheDir: resolve(options["--cache-dir"]) } : {}),
      },
    };
  }
  throw new ArtifactError("INVALID_COMMAND", `Unknown command ${JSON.stringify(command)}; expected stage, archive, or package.`);
}

export async function main(argv = process.argv.slice(2)) {
  const { command, options } = parseArgs(argv);
  if (command === "stage") {
    return stageReleaseArtifacts(options);
  }
  if (command === "archive") {
    return createArchive(options);
  }
  return packageReleaseArtifacts(options);
}

const invokedScript = process.argv[1];
if (invokedScript && import.meta.url === pathToFileURL(invokedScript).href) {
  main().catch((error) => {
    const code = error instanceof ArtifactError ? error.code : "RELEASE_ARTIFACTS_FAILED";
    process.stderr.write(`${error.name}: ${code}: ${error.message}\n`);
    process.exitCode = 1;
  });
}
