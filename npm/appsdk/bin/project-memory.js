#!/usr/bin/env node

import { LauncherError } from "../lib/errors.js";
import { runBinary } from "../lib/launcher.js";

try {
  await runBinary("project-memory");
} catch (error) {
  const detail =
    error instanceof LauncherError
      ? `${error.code}: ${error.message}`
      : (error?.stack ?? String(error));
  console.error(`project-memory: ${detail}`);
  process.exitCode = 1;
}
