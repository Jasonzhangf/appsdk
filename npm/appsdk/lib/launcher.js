import { spawn as spawnProcess } from "node:child_process";
import {
  accessSync,
  constants,
  readFileSync,
  statSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { LauncherError } from "./errors.js";
import {
  detectGlibcVersionRuntime,
  resolveBinaryPath,
  selectTarget,
} from "./target.js";

const require = createRequire(import.meta.url);
const MAIN_PACKAGE_PATH = fileURLToPath(
  new URL("../package.json", import.meta.url),
);
const MAIN_PACKAGE_NAME = "@jsonstudio/appsdk";

const SIGNALS_BY_PLATFORM = Object.freeze({
  darwin: Object.freeze(["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"]),
  linux: Object.freeze(["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"]),
  win32: Object.freeze(["SIGINT", "SIGTERM", "SIGBREAK"]),
});

export function readJsonFile(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

export function defaultResolvePackage(packageName) {
  try {
    return require.resolve(`${packageName}/package.json`);
  } catch (error) {
    throw new LauncherError(
      "PLATFORM_PACKAGE_MISSING",
      `Missing exact optional dependency ${packageName}; reinstall @jsonstudio/appsdk with optional dependencies enabled.`,
      { cause: error },
    );
  }
}

export function resolvePlatformRuntime({
  target,
  mainVersion,
  binaryName,
  resolvePackage = defaultResolvePackage,
  readPackage = readJsonFile,
  stat = statSync,
  access = accessSync,
}) {
  let packageJsonPath;
  try {
    packageJsonPath = resolvePackage(target.packageName);
  } catch (error) {
    if (error instanceof LauncherError) {
      throw error;
    }
    throw new LauncherError(
      "PLATFORM_PACKAGE_MISSING",
      `Could not resolve exact optional dependency ${target.packageName}.`,
      { cause: error },
    );
  }

  let platformPackage;
  try {
    platformPackage = readPackage(packageJsonPath);
  } catch (error) {
    throw new LauncherError(
      "PLATFORM_PACKAGE_INVALID",
      `Could not read platform package metadata at ${packageJsonPath}.`,
      { cause: error },
    );
  }

  if (platformPackage.name !== target.packageName) {
    throw new LauncherError(
      "PLATFORM_PACKAGE_NAME_MISMATCH",
      `Expected platform package ${target.packageName}, found ${JSON.stringify(platformPackage.name)}.`,
    );
  }

  if (platformPackage.version !== mainVersion) {
    throw new LauncherError(
      "PLATFORM_PACKAGE_VERSION_MISMATCH",
      `Platform package ${target.packageName} is version ${JSON.stringify(platformPackage.version)}, but @jsonstudio/appsdk requires ${JSON.stringify(mainVersion)}.`,
    );
  }

  const relativeBinary = resolveBinaryPath(target, binaryName);
  const binaryPath = join(dirname(packageJsonPath), relativeBinary);

  let fileInfo;
  try {
    fileInfo = stat(binaryPath);
  } catch (error) {
    throw new LauncherError(
      "PLATFORM_BINARY_MISSING",
      `Platform package ${target.packageName} is incomplete: expected verified binary at ${binaryPath}. M5 must assemble the M1/M2 artifact.`,
      { cause: error },
    );
  }

  if (!fileInfo.isFile()) {
    throw new LauncherError(
      "PLATFORM_BINARY_MISSING",
      `Platform package ${target.packageName} is incomplete: ${binaryPath} is not a file.`,
    );
  }

  if (target.platform !== "win32") {
    try {
      access(binaryPath, constants.X_OK);
    } catch (error) {
      throw new LauncherError(
        "PLATFORM_BINARY_NOT_EXECUTABLE",
        `Platform binary ${binaryPath} is not executable.`,
        { cause: error },
      );
    }
  }

  return Object.freeze({
    binaryPath,
    packageJsonPath,
    packageVersion: platformPackage.version,
  });
}

export function spawnBinary(
  binaryPath,
  argv,
  {
    spawn = spawnProcess,
    processObject = process,
    signalProcess = process.kill.bind(process),
    platform = process.platform,
  } = {},
) {
  if (!Array.isArray(argv)) {
    return Promise.reject(
      new LauncherError(
        "INVALID_ARGUMENTS",
        "Launcher arguments must be an array.",
      ),
    );
  }

  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(binaryPath, argv, {
        stdio: "inherit",
        shell: false,
        windowsHide: false,
      });
    } catch (error) {
      reject(
        new LauncherError(
          "PLATFORM_SPAWN_FAILED",
          `Failed to spawn ${binaryPath}: ${error.message}`,
          { cause: error },
        ),
      );
      return;
    }

    let settled = false;
    const signalHandlers = [];

    const cleanup = () => {
      for (const [signal, handler] of signalHandlers) {
        processObject.removeListener?.(signal, handler);
      }
    };

    const settleError = (error) => {
      if (settled) {
        return;
      }
      settled = true;
      cleanup();
      reject(
        new LauncherError(
          "PLATFORM_SPAWN_FAILED",
          `Failed to spawn ${binaryPath}: ${error.message}`,
          { cause: error },
        ),
      );
    };

    for (const signal of SIGNALS_BY_PLATFORM[platform] ?? []) {
      const handler = () => {
        try {
          child.kill(signal);
        } catch {
          // The child may already have exited; its close event owns the result.
        }
      };
      try {
        processObject.on(signal, handler);
        signalHandlers.push([signal, handler]);
      } catch {
        // Node does not expose every signal on every platform.
      }
    }

    child.once("error", settleError);
    child.once("close", (code, signal) => {
      if (settled) {
        return;
      }
      settled = true;
      cleanup();

      if (signal) {
        try {
          signalProcess(processObject.pid, signal);
          resolve({ code: null, signal });
        } catch (error) {
          reject(
            new LauncherError(
              "PLATFORM_SIGNAL_PROPAGATION_FAILED",
              `Child exited with ${signal}, but the launcher could not preserve that signal: ${error.message}`,
              { cause: error },
            ),
          );
        }
        return;
      }

      const exitCode = Number.isInteger(code) ? code : 1;
      processObject.exitCode = exitCode;
      resolve({ code: exitCode, signal: null });
    });
  });
}

export async function runBinary(
  binaryName,
  {
    argv = process.argv.slice(2),
    platform = process.platform,
    arch = process.arch,
    glibcVersionRuntime = detectGlibcVersionRuntime(),
    mainPackagePath = MAIN_PACKAGE_PATH,
    resolvePackage = defaultResolvePackage,
    readPackage = readJsonFile,
    stat = statSync,
    access = accessSync,
    spawn = spawnProcess,
    processObject = process,
    signalProcess = process.kill.bind(process),
  } = {},
) {
  const target = selectTarget({
    platform,
    arch,
    glibcVersionRuntime,
  });

  let mainPackage;
  try {
    mainPackage = readPackage(mainPackagePath);
  } catch (error) {
    throw new LauncherError(
      "MAIN_PACKAGE_INVALID",
      `Could not read @jsonstudio/appsdk package metadata at ${mainPackagePath}.`,
      { cause: error },
    );
  }

  if (mainPackage.name !== MAIN_PACKAGE_NAME) {
    throw new LauncherError(
      "MAIN_PACKAGE_NAME_MISMATCH",
      `Expected main package ${MAIN_PACKAGE_NAME}, found ${JSON.stringify(mainPackage.name)}.`,
    );
  }

  const runtime = resolvePlatformRuntime({
    target,
    mainVersion: mainPackage.version,
    binaryName,
    resolvePackage,
    readPackage,
    stat,
    access,
  });

  return spawnBinary(runtime.binaryPath, argv, {
    spawn,
    processObject,
    signalProcess,
    platform,
  });
}
