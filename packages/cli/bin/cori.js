#!/usr/bin/env node
// Runs the cori binary for this platform: the package @cori-do/cli-<platform>-<arch> that npm (pnpm, yarn, bun) installs
// as an optional dependency, with no install script, or the binary CORI_CLI_BINARY names (a build of your own). The
// arguments, the exit status and the signals pass through.
"use strict";
const { spawn } = require("child_process");
const fs = require("fs");

const BUILT = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"];

function fail(message) {
  console.error(`@cori-do/cli: ${message}`);
  process.exit(1);
}

function binary() {
  if (process.env.CORI_CLI_BINARY) return process.env.CORI_CLI_BINARY;
  const platform = `${process.platform}-${process.arch}`;
  if (process.platform === "win32") fail("there is no Windows build yet: install it in WSL (`npm install -g @cori-do/cli`)");
  if (!BUILT.includes(platform)) fail(`there is no build for ${platform} (only ${BUILT.join(", ")}): set CORI_CLI_BINARY to a cori you built (cargo install --path crates/cori-cli)`);
  let bin;
  try { bin = require.resolve(`@cori-do/cli-${platform}/bin/cori`); }
  catch { fail(`@cori-do/cli-${platform}, which holds the binary, is not installed: reinstall without --omit=optional (or --no-optional), or set CORI_CLI_BINARY`); }
  // an installer that dropped the executable bit
  try { fs.accessSync(bin, fs.constants.X_OK); } catch { try { fs.chmodSync(bin, 0o755); } catch {} }
  return bin;
}

const child = spawn(binary(), process.argv.slice(2), { stdio: "inherit" });
child.on("error", (e) => fail(`could not run ${e.path || "cori"}: ${e.message}`));
// Ctrl-C reaches the binary from the terminal, which signals the whole process group: this process only waits for it.
// A signal sent to this process alone is passed on.
process.on("SIGINT", () => {});
for (const signal of ["SIGTERM", "SIGHUP"]) process.on(signal, () => { try { child.kill(signal); } catch {} });
child.on("exit", (code, signal) => {
  if (!signal) process.exit(code ?? 1);
  // ended by a signal: end the same way, so that the shell sees it
  for (const s of ["SIGINT", "SIGTERM", "SIGHUP"]) process.removeAllListeners(s);
  process.kill(process.pid, signal);
});
