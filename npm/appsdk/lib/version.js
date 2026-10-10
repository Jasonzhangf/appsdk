import { LauncherError } from "./errors.js";

const SOURCE_VERSION_PATTERN = /^([0-9]+)\.([0-9]+)\.([0-9]{4})$/;
const CANDIDATE_SUFFIX = "-dev.0";

function normalizeComponent(component) {
  return BigInt(component).toString();
}

export function mapSourceVersion(sourceVersion) {
  if (typeof sourceVersion !== "string") {
    throw new LauncherError(
      "INVALID_SOURCE_VERSION",
      "AppSDK source version must be a string in MAJOR.MINOR.PPPP form.",
    );
  }

  const match = SOURCE_VERSION_PATTERN.exec(sourceVersion);
  if (!match) {
    throw new LauncherError(
      "INVALID_SOURCE_VERSION",
      `Invalid AppSDK source version ${JSON.stringify(sourceVersion)}; expected MAJOR.MINOR.PPPP.`,
    );
  }

  return match.slice(1).map(normalizeComponent).join(".");
}

export function candidateVersion(sourceVersion) {
  return `${mapSourceVersion(sourceVersion)}${CANDIDATE_SUFFIX}`;
}

export function assertCandidateVersion(sourceVersion, packageVersion) {
  const expected = candidateVersion(sourceVersion);
  if (packageVersion !== expected) {
    throw new LauncherError(
      "VERSION_MAPPING_MISMATCH",
      `Package version ${JSON.stringify(packageVersion)} does not match ${JSON.stringify(expected)} for source version ${JSON.stringify(sourceVersion)}.`,
    );
  }
  return expected;
}
