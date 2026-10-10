#!/usr/bin/env node

// Native tarball consumer acceptance for the AppSDK npm release.
//
// Given the packaged release directory (one main tarball, three platform
// tarballs, and SHA256SUMS), this script verifies the checksums, installs the
// exact local main + host-platform tarballs into an isolated project, verifies
// the installed platform package against artifact.json, and exercises the
// public npm entry points. It never downloads from the registry, never
// compiles, and never touches a shared Skill root or daemon.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import {
  basename,
  isAbsolute,
  join,
  posix,
  relative,
  resolve,
  win32,
} from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

import { selectTarget } from "../appsdk/lib/target.js";
import { SKILL_NAMES } from "./release-artifacts.js";

const CHECKSUMS_FILE = "SHA256SUMS";
const MAIN_TARBALL_PATTERN = /^jsonstudio-appsdk-[0-9]/;
const EXPECTED_TARBALL_COUNT = 4;
const RUNTIME_ROOTS = Object.freeze([
  "bin",
  ...SKILL_NAMES.map((name) => `skills/${name}`),
]);
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

export class ConsumerSmokeError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "ConsumerSmokeError";
    this.code = code;
  }
}

function sha256File(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function readJsonFile(filePath, code) {
  try {
    return JSON.parse(readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new ConsumerSmokeError(code, `Could not read JSON ${filePath}: ${error.message}`, { cause: error });
  }
}

function isSafeArtifactPath(value) {
  return typeof value === "string"
    && value.length > 0
    && !value.includes("\\")
    && !value.startsWith("/")
    && value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function walkRuntimeFiles(directory, prefix, files) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const entryPath = join(directory, entry.name);
    const entryRelative = `${prefix}/${entry.name}`;
    if (entry.isDirectory()) {
      walkRuntimeFiles(entryPath, entryRelative, files);
    } else if (entry.isFile()) {
      files.push(entryRelative);
    } else {
      throw new ConsumerSmokeError(
        "INSTALLED_FILE_TYPE_MISMATCH",
        `Installed platform entry ${entryPath} is not a regular file or directory.`,
      );
    }
  }
}

function listRuntimeFiles(platformPackageDir) {
  const files = [];
  for (const root of RUNTIME_ROOTS) {
    const rootPath = join(platformPackageDir, ...root.split("/"));
    let info;
    try {
      info = lstatSync(rootPath);
    } catch (error) {
      throw new ConsumerSmokeError(
        "INSTALLED_RUNTIME_MISSING",
        `Installed platform runtime root ${rootPath} is missing.`,
        { cause: error },
      );
    }
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new ConsumerSmokeError(
        "INSTALLED_RUNTIME_MISSING",
        `Installed platform runtime root ${rootPath} is not a regular directory.`,
      );
    }
    walkRuntimeFiles(rootPath, root, files);
  }
  return files.sort();
}

export function verifyInstalledArtifact(platformPackageDir, expected = {}) {
  const artifactPath = join(platformPackageDir, "artifact.json");
  const artifact = readJsonFile(artifactPath, "ARTIFACT_MANIFEST_MISSING");
  if (artifact.targetTriple !== expected.targetTriple) {
    throw new ConsumerSmokeError(
      "ARTIFACT_TARGET_MISMATCH",
      `Installed artifact target ${JSON.stringify(artifact.targetTriple)} does not match ${JSON.stringify(expected.targetTriple)}.`,
    );
  }
  if (expected.sourceVersion !== undefined && artifact.sourceVersion !== expected.sourceVersion) {
    throw new ConsumerSmokeError(
      "ARTIFACT_SOURCE_VERSION_MISMATCH",
      `Installed artifact source version ${JSON.stringify(artifact.sourceVersion)} does not match ${JSON.stringify(expected.sourceVersion)}.`,
    );
  }
  if (!artifact.files || typeof artifact.files !== "object" || Array.isArray(artifact.files)) {
    throw new ConsumerSmokeError("ARTIFACT_MANIFEST_INVALID", "Installed artifact.files must be an object.");
  }

  const expectedFiles = Object.keys(artifact.files).sort();
  const actualFiles = listRuntimeFiles(platformPackageDir);
  if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
    throw new ConsumerSmokeError(
      "INSTALLED_FILE_SET_MISMATCH",
      `Installed platform files ${JSON.stringify(actualFiles)} do not match artifact manifest ${JSON.stringify(expectedFiles)}.`,
    );
  }

  for (const [relativePath, expectedHash] of Object.entries(artifact.files)) {
    if (!isSafeArtifactPath(relativePath) || !/^[0-9a-f]{64}$/.test(expectedHash)) {
      throw new ConsumerSmokeError(
        "ARTIFACT_MANIFEST_INVALID",
        `Installed artifact entry ${JSON.stringify(relativePath)} is invalid.`,
      );
    }
    const filePath = join(platformPackageDir, ...relativePath.split("/"));
    let actualHash;
    try {
      actualHash = sha256File(filePath);
    } catch (error) {
      throw new ConsumerSmokeError(
        "INSTALLED_FILE_MISSING",
        `Installed platform file ${filePath} is missing.`,
        { cause: error },
      );
    }
    if (actualHash !== expectedHash) {
      throw new ConsumerSmokeError(
        "INSTALLED_HASH_MISMATCH",
        `Installed platform file ${relativePath} has SHA-256 ${actualHash}, expected ${expectedHash}.`,
      );
    }
  }
  return artifact;
}

export function readChecksums(packageDir) {
  const checksumsPath = join(packageDir, CHECKSUMS_FILE);
  let text;
  try {
    text = readFileSync(checksumsPath, "utf8");
  } catch (error) {
    throw new ConsumerSmokeError("CHECKSUMS_MISSING", `Missing ${checksumsPath}.`, { cause: error });
  }
  const checksums = new Map();
  for (const line of text.split("\n")) {
    if (line.trim() === "") {
      continue;
    }
    const match = /^([0-9a-f]{64}) {2}(.+)$/.exec(line);
    if (!match) {
      throw new ConsumerSmokeError("CHECKSUMS_INVALID", `Invalid ${CHECKSUMS_FILE} line ${JSON.stringify(line)}.`);
    }
    if (checksums.has(match[2])) {
      throw new ConsumerSmokeError("CHECKSUMS_INVALID", `Duplicate ${CHECKSUMS_FILE} entry ${JSON.stringify(match[2])}.`);
    }
    checksums.set(match[2], match[1]);
  }
  if (checksums.size === 0) {
    throw new ConsumerSmokeError("CHECKSUMS_INVALID", `${CHECKSUMS_FILE} has no entries.`);
  }
  return checksums;
}

export function selectTarballs(packageDir, target) {
  const checksums = readChecksums(packageDir);
  const tarballs = readdirSync(packageDir).filter((name) => name.endsWith(".tgz")).sort();
  if (tarballs.length !== EXPECTED_TARBALL_COUNT) {
    throw new ConsumerSmokeError(
      "TARBALL_SET_MISMATCH",
      `Expected exactly ${EXPECTED_TARBALL_COUNT} tarballs, found ${tarballs.length}: ${tarballs.join(", ")}.`,
    );
  }
  for (const name of tarballs) {
    const expected = checksums.get(name);
    if (expected === undefined) {
      throw new ConsumerSmokeError("CHECKSUMS_MISSING", `${CHECKSUMS_FILE} has no entry for ${name}.`);
    }
    const actual = sha256File(join(packageDir, name));
    if (actual !== expected) {
      throw new ConsumerSmokeError("CHECKSUM_MISMATCH", `Tarball ${name} has SHA-256 ${actual}, expected ${expected}.`);
    }
  }
  const platformKey = target.packageName.split("/")[1];
  const mainTarballs = tarballs.filter((name) => MAIN_TARBALL_PATTERN.test(name));
  const platformTarballs = tarballs.filter((name) => name.startsWith(`jsonstudio-${platformKey}-`));
  if (mainTarballs.length !== 1) {
    throw new ConsumerSmokeError("MAIN_TARBALL_MISSING", `Expected one main tarball, found ${mainTarballs.length}: ${mainTarballs.join(", ")}.`);
  }
  if (platformTarballs.length !== 1) {
    throw new ConsumerSmokeError("PLATFORM_TARBALL_MISSING", `Expected one ${platformKey} tarball, found ${platformTarballs.length}: ${platformTarballs.join(", ")}.`);
  }
  return {
    mainTarball: join(packageDir, mainTarballs[0]),
    platformTarball: join(packageDir, platformTarballs[0]),
    tarballs,
  };
}

function npmCliCandidates(execPath, cliFile, platform) {
  const pathApi = platform === "win32" ? win32 : posix;
  const binDir = pathApi.dirname(execPath);
  return [
    pathApi.join(binDir, "node_modules", "npm", "bin", cliFile),
    pathApi.join(binDir, "..", "lib", "node_modules", "npm", "bin", cliFile),
    pathApi.join(binDir, "..", "libexec", "node_modules", "npm", "bin", cliFile),
  ];
}

export function resolveNpmCli(
  name,
  {
    platform = process.platform,
    execPath = process.execPath,
    fileExists = existsSync,
  } = {},
) {
  if (name !== "npm" && name !== "npx") {
    throw new ConsumerSmokeError("INVALID_NPM_CLI", `Unknown npm CLI ${JSON.stringify(name)}.`);
  }
  const cliFile = name === "npm" ? "npm-cli.js" : "npx-cli.js";
  const candidates = npmCliCandidates(execPath, cliFile, platform);
  for (const candidate of candidates) {
    if (fileExists(candidate)) {
      return { command: execPath, args: [candidate] };
    }
  }
  if (platform === "win32") {
    throw new ConsumerSmokeError(
      "NPM_CLI_MISSING",
      `Could not find ${cliFile} next to ${execPath}; checked ${candidates.join(", ")}.`,
    );
  }
  return { command: name, args: [] };
}

export function npmInvocation(options) {
  return resolveNpmCli("npm", options);
}

export function npxInvocation(options) {
  return resolveNpmCli("npx", options);
}

function quoteCmdArgument(value) {
  if (value === "") {
    return '""';
  }
  return /[\s"&|<>^]/.test(value)
    ? `"${value.replaceAll('"', '""')}"`
    : value;
}

export function localShimInvocation(
  projectDir,
  name,
  args,
  {
    platform = process.platform,
    comspec = process.env.ComSpec ?? process.env.COMSPEC ?? "cmd.exe",
  } = {},
) {
  const pathApi = platform === "win32" ? win32 : posix;
  const shimPath = pathApi.join(
    projectDir,
    "node_modules",
    ".bin",
    platform === "win32" ? `${name}.cmd` : name,
  );
  if (platform !== "win32") {
    return { command: shimPath, args: [...args] };
  }
  const commandLine = [shimPath, ...args].map(quoteCmdArgument).join(" ");
  return {
    command: comspec,
    args: ["/d", "/s", "/c", `"${commandLine}"`],
  };
}

function isolatedEnv(root) {
  const home = join(root, "home");
  const temp = join(root, "tmp");
  const paths = {
    home,
    temp,
    appData: join(home, "AppData", "Roaming"),
    localAppData: join(home, "AppData", "Local"),
    appsdkHome: join(root, "appsdk-home"),
    codexHome: join(root, "codex-home"),
    collabState: join(root, "collab-state"),
    xdgConfig: join(home, ".config"),
    xdgData: join(home, ".local", "share"),
    npmCache: join(root, "npm-cache"),
    npmPrefix: join(root, "npm-prefix"),
    npmUserConfig: join(root, "npmrc"),
  };
  for (const directory of [
    paths.home,
    paths.temp,
    paths.appData,
    paths.localAppData,
    paths.appsdkHome,
    paths.codexHome,
    paths.collabState,
    paths.xdgConfig,
    paths.xdgData,
    paths.npmCache,
    paths.npmPrefix,
  ]) {
    mkdirSync(directory, { recursive: true });
  }
  writeFileSync(paths.npmUserConfig, "");
  const env = {
    ...process.env,
    HOME: paths.home,
    USERPROFILE: paths.home,
    APPDATA: paths.appData,
    LOCALAPPDATA: paths.localAppData,
    XDG_CONFIG_HOME: paths.xdgConfig,
    XDG_DATA_HOME: paths.xdgData,
    APPSDK_HOME: paths.appsdkHome,
    CODEX_HOME: paths.codexHome,
    COLLAB_STATE_DIR: paths.collabState,
    TMPDIR: paths.temp,
    TEMP: paths.temp,
    TMP: paths.temp,
    npm_config_cache: paths.npmCache,
    npm_config_prefix: paths.npmPrefix,
    npm_config_userconfig: paths.npmUserConfig,
    npm_config_offline: "true",
    npm_config_audit: "false",
    npm_config_fund: "false",
    npm_config_update_notifier: "false",
  };
  delete env.npm_execpath;
  return env;
}

function assertEnvUnderRoot(root, env) {
  for (const key of [
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "APPSDK_HOME",
    "CODEX_HOME",
    "COLLAB_STATE_DIR",
    "TMPDIR",
    "TEMP",
    "TMP",
    "npm_config_cache",
    "npm_config_prefix",
    "npm_config_userconfig",
  ]) {
    const value = env[key];
    if (value === undefined) {
      continue;
    }
    const relativePath = relative(resolve(root), resolve(value));
    if (relativePath === "" || relativePath.startsWith("..") || isAbsolute(relativePath)) {
      throw new ConsumerSmokeError(
        "ISOLATION_MISMATCH",
        `${key}=${value} is outside isolated root ${root}.`,
      );
    }
  }
}

export function runCommand(command, args, { cwd, env } = {}) {
  return spawnSync(command, args, {
    cwd,
    env,
    encoding: "utf8",
    shell: false,
  });
}

function formatCommandResult(result) {
  const lines = [];
  if (result.error) {
    lines.push(`error.code=${result.error.code ?? "<none>"}`);
    lines.push(`error.message=${result.error.message}`);
  }
  lines.push(`status=${result.status === null ? "null" : result.status}`);
  if (result.signal) {
    lines.push(`signal=${result.signal}`);
  }
  if (result.stderr) {
    lines.push(`stderr:\n${result.stderr.trimEnd()}`);
  }
  return lines.join("\n");
}

export function assertCommandSucceeded(result, code, label) {
  if (result.error || result.status !== 0) {
    throw new ConsumerSmokeError(
      code,
      `${label} failed.\n${formatCommandResult(result)}`,
      { cause: result.error },
    );
  }
  return result;
}

function assertCommandFailedWith(result, expectedCode, code, label) {
  if (result.error) {
    throw new ConsumerSmokeError(
      code,
      `${label} could not launch.\n${formatCommandResult(result)}`,
      { cause: result.error },
    );
  }
  if (result.status === 0) {
    throw new ConsumerSmokeError(
      `${code}_PASSTHROUGH`,
      `${label} unexpectedly succeeded.\n${formatCommandResult(result)}`,
    );
  }
  if (!(result.stderr ?? "").includes(expectedCode)) {
    throw new ConsumerSmokeError(
      `${code}_UNEXPECTED`,
      `${label} did not report ${expectedCode}.\n${formatCommandResult(result)}`,
    );
  }
  return result;
}

function runLocalShim(projectDir, name, args, env) {
  const invocation = localShimInvocation(projectDir, name, args);
  return runCommand(invocation.command, invocation.args, { cwd: projectDir, env });
}

function installSpecs(npm, projectDir, env, specs) {
  const result = runCommand(
    npm.command,
    [
      ...npm.args,
      "install",
      "--no-save",
      "--no-package-lock",
      "--ignore-scripts",
      "--omit=optional",
      "--offline",
      "--no-audit",
      "--no-fund",
      ...specs,
    ],
    { cwd: projectDir, env },
  );
  return assertCommandSucceeded(result, "INSTALL_FAILED", `npm install (${specs.join(", ")})`);
}

function uninstallSpecs(npm, projectDir, env, specs) {
  const result = runCommand(
    npm.command,
    [
      ...npm.args,
      "uninstall",
      "--no-save",
      "--no-package-lock",
      "--ignore-scripts",
      "--offline",
      ...specs,
    ],
    { cwd: projectDir, env },
  );
  return assertCommandSucceeded(result, "UNINSTALL_FAILED", `npm uninstall (${specs.join(", ")})`);
}

function expectedAppsdkVersion(sourceVersion) {
  return `appsdk ${sourceVersion} (rust)`;
}

function assertAppsdkVersion(result, sourceVersion, label) {
  assertCommandSucceeded(result, `${label.toUpperCase().replaceAll(" ", "_")}_FAILED`, label);
  const actual = (result.stdout ?? "").trim();
  const expected = expectedAppsdkVersion(sourceVersion);
  if (actual !== expected) {
    throw new ConsumerSmokeError(
      "APPSDK_VERSION_OUTPUT_MISMATCH",
      `${label} produced ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}.`,
    );
  }
  return actual;
}

function assertMemoryHelp(result, label) {
  assertCommandSucceeded(result, `${label.toUpperCase().replaceAll(" ", "_")}_FAILED`, label);
  let parsed;
  try {
    parsed = JSON.parse(result.stdout.trim());
  } catch (error) {
    throw new ConsumerSmokeError(
      "MEMORY_HELP_OUTPUT_INVALID",
      `${label} did not return JSON: ${error.message}\n${formatCommandResult(result)}`,
      { cause: error },
    );
  }
  if (!Array.isArray(parsed.commands) || !parsed.commands.includes("entry") || !parsed.commands.includes("query") || !parsed.commands.includes("verify")) {
    throw new ConsumerSmokeError(
      "MEMORY_HELP_OUTPUT_MISMATCH",
      `${label} did not list the expected project-memory commands: ${JSON.stringify(parsed)}.`,
    );
  }
  if (typeof parsed.query_hint !== "string" || !parsed.query_hint.includes("project-memory")) {
    throw new ConsumerSmokeError(
      "MEMORY_HELP_OUTPUT_MISMATCH",
      `${label} did not expose the expected query hint: ${JSON.stringify(parsed)}.`,
    );
  }
}

function assertGovernanceResources(projectDir) {
  for (const relativePath of GOVERNANCE_DIRS) {
    const resourcePath = join(projectDir, ...relativePath.split("/"));
    let info;
    try {
      info = statSync(resourcePath);
    } catch (error) {
      throw new ConsumerSmokeError(
        "GOVERNANCE_RESOURCE_MISSING",
        `appsdk new did not create directory ${relativePath}.`,
        { cause: error },
      );
    }
    if (!info.isDirectory()) {
      throw new ConsumerSmokeError(
        "GOVERNANCE_RESOURCE_TYPE_MISMATCH",
        `appsdk new created ${relativePath}, but it is not a directory.`,
      );
    }
  }
  for (const relativePath of GOVERNANCE_FILES) {
    const resourcePath = join(projectDir, ...relativePath.split("/"));
    let info;
    try {
      info = statSync(resourcePath);
    } catch (error) {
      throw new ConsumerSmokeError(
        "GOVERNANCE_RESOURCE_MISSING",
        `appsdk new did not create file ${relativePath}.`,
        { cause: error },
      );
    }
    if (!info.isFile()) {
      throw new ConsumerSmokeError(
        "GOVERNANCE_RESOURCE_TYPE_MISMATCH",
        `appsdk new created ${relativePath}, but it is not a file.`,
      );
    }
  }
}

function assertRegistration(stdout, sourceVersion) {
  const line = stdout
    .split(/\r?\n/)
    .find((candidate) => candidate.startsWith("appsdk-registration "));
  if (!line) {
    throw new ConsumerSmokeError(
      "REGISTRATION_RECEIPT_MISSING",
      `appsdk new did not emit an appsdk-registration receipt.\n${stdout}`,
    );
  }
  let receipt;
  try {
    receipt = JSON.parse(line.slice("appsdk-registration ".length));
  } catch (error) {
    throw new ConsumerSmokeError(
      "REGISTRATION_RECEIPT_INVALID",
      `appsdk new emitted an invalid registration receipt: ${error.message}`,
      { cause: error },
    );
  }
  if (receipt.sdk_version !== sourceVersion || receipt.idempotent !== false) {
    throw new ConsumerSmokeError(
      "REGISTRATION_RECEIPT_MISMATCH",
      `appsdk new registration receipt does not match source version ${sourceVersion}: ${JSON.stringify(receipt)}.`,
    );
  }
}

function assertVerify(stdout) {
  let parsed;
  try {
    parsed = JSON.parse(stdout.trim());
  } catch (error) {
    throw new ConsumerSmokeError(
      "VERIFY_OUTPUT_INVALID",
      `appsdk verify did not return JSON: ${error.message}\n${stdout}`,
      { cause: error },
    );
  }
  if (parsed.command_ok !== true || parsed.development_ready !== true) {
    throw new ConsumerSmokeError(
      "VERIFY_OUTPUT_MISMATCH",
      `appsdk verify did not report command_ok/development_ready: ${JSON.stringify(parsed)}.`,
    );
  }
}

export function runConsumerSmoke({ packageDir, targetTriple, baseDir, log = () => {} }) {
  const target = selectTarget();
  if (targetTriple !== undefined && targetTriple !== target.targetTriple) {
    throw new ConsumerSmokeError(
      "TARGET_MISMATCH",
      `Requested target ${JSON.stringify(targetTriple)} does not match the native host target ${JSON.stringify(target.targetTriple)}.`,
    );
  }
  const { mainTarball, platformTarball } = selectTarballs(packageDir, target);

  const root = mkdtempSync(join(baseDir ?? tmpdir(), "appsdk consumer "));
  const projectDir = join(root, "consumer project");
  const disabledProjectDir = join(root, "disabled optional");
  const governedProjectDir = join(root, "governed project");
  const markerPath = join(projectDir, "user-owned-marker.txt");
  const markerContents = "keep me\n";
  for (const directory of [projectDir, disabledProjectDir, governedProjectDir]) {
    mkdirSync(directory, { recursive: true });
  }
  writeFileSync(
    join(projectDir, "package.json"),
    `${JSON.stringify({ name: "appsdk-consumer-smoke", version: "0.0.0", private: true }, null, 2)}\n`,
  );
  writeFileSync(
    join(disabledProjectDir, "package.json"),
    `${JSON.stringify({ name: "appsdk-disabled-optional-smoke", version: "0.0.0", private: true }, null, 2)}\n`,
  );
  writeFileSync(markerPath, markerContents);

  const env = isolatedEnv(root);
  assertEnvUnderRoot(root, env);
  const npm = npmInvocation();
  const npx = npxInvocation();

  try {
    log(`installing ${basename(platformTarball)} then ${basename(mainTarball)}`);
    installSpecs(npm, projectDir, env, [platformTarball, mainTarball]);

    const mainPackageDir = join(projectDir, "node_modules", "@jsonstudio", "appsdk");
    const platformPackageDir = join(projectDir, "node_modules", ...target.packageName.split("/"));
    const mainManifestPath = join(mainPackageDir, "package.json");
    const platformManifestPath = join(platformPackageDir, "package.json");
    const mainManifest = readJsonFile(mainManifestPath, "MAIN_PACKAGE_MISSING");
    const platformManifest = readJsonFile(platformManifestPath, "PLATFORM_PACKAGE_MISSING");
    const sourceVersion = mainManifest.appsdk?.sourceVersion;
    if (typeof sourceVersion !== "string" || sourceVersion === "") {
      throw new ConsumerSmokeError(
        "MAIN_SOURCE_VERSION_MISSING",
        `Main package ${mainManifestPath} does not declare appsdk.sourceVersion.`,
      );
    }
    if (mainManifest.name !== "@jsonstudio/appsdk") {
      throw new ConsumerSmokeError(
        "MAIN_PACKAGE_MISMATCH",
        `Installed main package ${JSON.stringify(mainManifest.name)} is not @jsonstudio/appsdk.`,
      );
    }
    if (platformManifest.name !== target.packageName) {
      throw new ConsumerSmokeError("PLATFORM_PACKAGE_MISMATCH", `Installed platform package ${JSON.stringify(platformManifest.name)} is not ${JSON.stringify(target.packageName)}.`);
    }
    if (platformManifest.version !== mainManifest.version) {
      throw new ConsumerSmokeError("PLATFORM_VERSION_MISMATCH", `Platform package version ${JSON.stringify(platformManifest.version)} does not match main package ${JSON.stringify(mainManifest.version)}.`);
    }
    const artifact = verifyInstalledArtifact(platformPackageDir, {
      targetTriple: target.targetTriple,
      sourceVersion,
    });

    log("running both public entries through local npm shims");
    const shimAppsdkRun = runLocalShim(projectDir, "appsdk", ["version"], env);
    const appsdkVersion = assertAppsdkVersion(shimAppsdkRun, sourceVersion, "local appsdk shim");
    const shimMemoryRun = runLocalShim(projectDir, "project-memory", ["help"], env);
    assertMemoryHelp(shimMemoryRun, "local project-memory shim");

    log("running the public entries through npm exec and npx");
    const execRun = runCommand(
      npm.command,
      [...npm.args, "exec", "--offline", "--", "appsdk", "version"],
      { cwd: projectDir, env },
    );
    assertAppsdkVersion(execRun, sourceVersion, "npm exec appsdk");
    const npxRun = runCommand(
      npx.command,
      [...npx.args, "--offline", "project-memory", "help"],
      { cwd: projectDir, env },
    );
    assertMemoryHelp(npxRun, "npx project-memory");

    log("creating and verifying a real project through the installed appsdk shim");
    const newRun = runLocalShim(projectDir, "appsdk", ["new", governedProjectDir], env);
    assertCommandSucceeded(newRun, "APPSDK_NEW_FAILED", "appsdk new");
    assertRegistration(newRun.stdout ?? "", sourceVersion);
    assertGovernanceResources(governedProjectDir);
    const verifyRun = runLocalShim(projectDir, "appsdk", ["verify", governedProjectDir], env);
    assertCommandSucceeded(verifyRun, "APPSDK_VERIFY_FAILED", "appsdk verify");
    assertVerify(verifyRun.stdout ?? "");

    log("checking disabled optional dependency, missing binary, and version mismatch failures");
    installSpecs(npm, disabledProjectDir, env, [mainTarball]);
    const disabledRun = runLocalShim(disabledProjectDir, "appsdk", ["version"], env);
    assertCommandFailedWith(
      disabledRun,
      "PLATFORM_PACKAGE_MISSING",
      "DISABLED_OPTIONAL_ERROR",
      "appsdk with optional dependencies disabled",
    );

    const binaryPath = join(platformPackageDir, ...target.binaries.appsdk.split("/"));
    unlinkSync(binaryPath);
    const missingRun = runLocalShim(projectDir, "appsdk", ["version"], env);
    assertCommandFailedWith(
      missingRun,
      "PLATFORM_BINARY_MISSING",
      "MISSING_BINARY_ERROR",
      "appsdk with the platform binary removed",
    );

    const mismatchedPlatformManifest = {
      ...readJsonFile(platformManifestPath, "PLATFORM_PACKAGE_MISSING"),
      version: `${mainManifest.version}-mismatch`,
    };
    writeFileSync(platformManifestPath, `${JSON.stringify(mismatchedPlatformManifest, null, 2)}\n`);
    const mismatchRun = runLocalShim(projectDir, "appsdk", ["version"], env);
    assertCommandFailedWith(
      mismatchRun,
      "PLATFORM_PACKAGE_VERSION_MISMATCH",
      "VERSION_MISMATCH_ERROR",
      "appsdk with a mismatched platform package version",
    );

    log("removing the npm-managed tree and reinstalling the same accepted tarballs");
    rmSync(join(projectDir, "node_modules"), { recursive: true, force: true });
    installSpecs(npm, projectDir, env, [platformTarball, mainTarball]);
    verifyInstalledArtifact(platformPackageDir, {
      targetTriple: target.targetTriple,
      sourceVersion,
    });
    const reinstalledRun = runLocalShim(projectDir, "appsdk", ["version"], env);
    assertAppsdkVersion(reinstalledRun, sourceVersion, "reinstalled appsdk shim");

    log("uninstalling npm-managed packages while preserving project state");
    uninstallSpecs(npm, projectDir, env, [target.packageName, mainManifest.name]);
    if (existsSync(mainPackageDir) || existsSync(platformPackageDir)) {
      throw new ConsumerSmokeError(
        "UNINSTALL_INCOMPLETE",
        `npm uninstall left managed packages in ${projectDir}.`,
      );
    }
    if (!existsSync(markerPath) || readFileSync(markerPath, "utf8") !== markerContents) {
      throw new ConsumerSmokeError(
        "USER_MARKER_LOST",
        `Uninstall changed the user-owned marker ${markerPath}.`,
      );
    }
    if (!existsSync(governedProjectDir) || !existsSync(join(governedProjectDir, ".appsdk", "sdk-resources.json"))) {
      throw new ConsumerSmokeError(
        "CONSUMER_PROJECT_LOST",
        `Uninstall removed the consumer project ${governedProjectDir}.`,
      );
    }

    rmSync(root, { recursive: true, force: true });
    return { targetTriple: target.targetTriple, appsdkVersion, projectDir, sourceVersion, artifact };
  } catch (error) {
    log(`consumer smoke failed; isolated workspace kept at ${root}`);
    throw error;
  }
}

export function parseArgs(argv) {
  const allowed = ["--package-dir", "--target-triple", "--base-dir"];
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || value === undefined || options[flag] !== undefined) {
      throw new ConsumerSmokeError("INVALID_ARGUMENTS", `Invalid arguments ${JSON.stringify(argv)}; expected ${allowed.join(" ")}.`);
    }
    options[flag] = value;
  }
  if (options["--package-dir"] === undefined) {
    throw new ConsumerSmokeError("INVALID_ARGUMENTS", "Missing required argument --package-dir.");
  }
  return {
    packageDir: resolve(options["--package-dir"]),
    ...(options["--target-triple"] === undefined ? {} : { targetTriple: options["--target-triple"] }),
    ...(options["--base-dir"] === undefined ? {} : { baseDir: resolve(options["--base-dir"]) }),
  };
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const result = runConsumerSmoke({ ...options, log: (line) => process.stdout.write(`${line}\n`) });
  process.stdout.write(`consumer smoke PASS: ${result.targetTriple} (${result.appsdkVersion})\n`);
  return result;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    const code = error instanceof ConsumerSmokeError ? error.code : "CONSUMER_SMOKE_FAILED";
    process.stderr.write(`${error.name}: ${code}: ${error.message}\n`);
    process.exitCode = 1;
  }
}
