import { LauncherError } from "./errors.js";

const BINARY_NAMES = Object.freeze(["appsdk", "project-memory"]);

function binaries(platform) {
  const suffix = platform === "win32" ? ".exe" : "";
  return Object.freeze({
    appsdk: `bin/appsdk${suffix}`,
    "project-memory": `bin/project-memory${suffix}`,
  });
}

const TARGETS = Object.freeze({
  "darwin:arm64": Object.freeze({
    platform: "darwin",
    arch: "arm64",
    targetTriple: "aarch64-apple-darwin",
    packageName: "@jsonstudio/appsdk-darwin-arm64",
    binaries: binaries("darwin"),
  }),
  "linux:x64": Object.freeze({
    platform: "linux",
    arch: "x64",
    targetTriple: "x86_64-unknown-linux-gnu",
    packageName: "@jsonstudio/appsdk-linux-x64-gnu",
    libc: "glibc",
    binaries: binaries("linux"),
  }),
  "win32:x64": Object.freeze({
    platform: "win32",
    arch: "x64",
    targetTriple: "x86_64-pc-windows-msvc",
    packageName: "@jsonstudio/appsdk-win32-x64-msvc",
    binaries: binaries("win32"),
  }),
});

export const SUPPORTED_TARGETS = Object.freeze(Object.values(TARGETS));
export const SUPPORTED_BINARY_NAMES = BINARY_NAMES;

export function detectGlibcVersionRuntime() {
  try {
    return process.report?.getReport?.().header?.glibcVersionRuntime;
  } catch {
    return undefined;
  }
}

export function selectTarget({
  platform = process.platform,
  arch = process.arch,
  glibcVersionRuntime = detectGlibcVersionRuntime(),
} = {}) {
  if (platform === "linux" && arch === "x64") {
    if (!glibcVersionRuntime) {
      throw new LauncherError(
        "UNSUPPORTED_ABI",
        "AppSDK npm supports Linux x64 with glibc only; musl and unknown Linux ABIs are not supported.",
      );
    }
    return TARGETS["linux:x64"];
  }

  const target = TARGETS[`${platform}:${arch}`];
  if (target) {
    return target;
  }

  if (!["darwin", "linux", "win32"].includes(platform)) {
    throw new LauncherError(
      "UNSUPPORTED_PLATFORM",
      `AppSDK npm does not support platform ${platform}; supported platforms are darwin, linux, and win32.`,
    );
  }

  throw new LauncherError(
    "UNSUPPORTED_CPU",
    `AppSDK npm does not support ${platform}/${arch}; supported targets are darwin/arm64, linux/x64 (glibc), and win32/x64.`,
  );
}

export function resolveBinaryPath(target, binaryName) {
  const relativePath = target.binaries[binaryName];
  if (!relativePath) {
    throw new LauncherError(
      "UNKNOWN_BINARY",
      `Unknown AppSDK launcher ${binaryName}; expected one of ${BINARY_NAMES.join(", ")}.`,
    );
  }
  return relativePath;
}
