import assert from "node:assert/strict";
import test from "node:test";

import { LauncherError } from "../lib/errors.js";
import {
  assertCandidateVersion,
  candidateVersion,
  mapSourceVersion,
} from "../lib/version.js";

test("maps source versions by decimal component normalization", () => {
  assert.equal(mapSourceVersion("0.1.0014"), "0.1.14");
  assert.equal(mapSourceVersion("0.1.0015"), "0.1.15");
  assert.equal(mapSourceVersion("00.01.0014"), "0.1.14");
  assert.equal(mapSourceVersion("1.02.0000"), "1.2.0");
  assert.equal(candidateVersion("0.1.0014"), "0.1.14-dev.0");
  assert.equal(candidateVersion("0.1.0015"), "0.1.15-dev.0");
});

test("rejects non-canonical source version spellings", () => {
  for (const sourceVersion of ["0.1.14", "0.1.0014-dev", "0.1.001", "v0.1.0014"]) {
    assert.throws(
      () => mapSourceVersion(sourceVersion),
      (error) =>
        error instanceof LauncherError &&
        error.code === "INVALID_SOURCE_VERSION",
    );
  }
});

test("checks candidate metadata versions", () => {
  assert.equal(
    assertCandidateVersion("0.1.0014", "0.1.14-dev.0"),
    "0.1.14-dev.0",
  );
  assert.throws(
    () => assertCandidateVersion("0.1.0014", "0.1.14"),
    (error) =>
      error instanceof LauncherError &&
      error.code === "VERSION_MAPPING_MISMATCH",
  );
});
