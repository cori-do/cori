#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Assembles the npm packages of a cli release from its built binaries:
//   node packages/cli/pack.mjs --out <dir> --notices <THIRD-PARTY-NOTICES.md> [--release] <target>=<binary>…
// - <out>/cli-<os>-<cpu>: @cori-do/cli-<os>-<cpu>, one binary, installed only where npm's `os` and `cpu` match;
// - <out>/cli: @cori-do/cli, the launcher (bin/cori.js), with those packages as optional dependencies of its version.
// Every package carries the licence, its NOTICE and the third-party notices (scripts/cli-notices.sh). The version is
// packages/cli/package.json's, which must be the workspace's (Cargo.toml). `--release` requires a binary for every
// platform. Publish the platform packages first, then cli (.github/workflows/release-cli.yml).
import { chmodSync, copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");

/** The release targets, and the `os` and `cpu` of their packages. */
const PLATFORMS = {
  "aarch64-apple-darwin": ["darwin", "arm64"],
  "x86_64-apple-darwin": ["darwin", "x64"],
  "aarch64-unknown-linux-musl": ["linux", "arm64"],
  "x86_64-unknown-linux-musl": ["linux", "x64"],
};
const OS_NAMES = { darwin: "macOS", linux: "Linux" };

const args = process.argv.slice(2);
function option(name) {
  const i = args.indexOf(name);
  if (i < 0 || !args[i + 1]) throw new Error(`${name} is required`);
  return args.splice(i, 2)[1];
}
function flag(name) {
  const i = args.indexOf(name);
  return i >= 0 && args.splice(i, 1).length > 0;
}
const out = resolve(option("--out"));
const notices = resolve(option("--notices"));
const release = flag("--release");
const binaries = args.map((a) => {
  const [target, file] = a.split("=");
  if (!PLATFORMS[target] || !file) throw new Error(`not <target>=<binary> with a target of ${Object.keys(PLATFORMS).join(", ")}: ${a}`);
  return { target, file: resolve(file) };
});
if (!binaries.length) throw new Error("no binary given");
if (release && binaries.length !== Object.keys(PLATFORMS).length) throw new Error(`a release has a binary for each of ${Object.keys(PLATFORMS).join(", ")}`);

const main = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));
const workspace = /\[workspace\.package\][^[]*?\bversion\s*=\s*"([^"]+)"/.exec(readFileSync(join(repo, "Cargo.toml"), "utf8"))?.[1];
if (main.version !== workspace) throw new Error(`packages/cli/package.json is at ${main.version} and the workspace at ${workspace}: bump both`);

rmSync(out, { recursive: true, force: true });
const shared = { license: main.license, homepage: main.homepage, repository: main.repository };
function finish(dir, manifest) {
  copyFileSync(join(repo, "LICENSE"), join(dir, "LICENSE"));
  copyFileSync(join(repo, "NOTICE"), join(dir, "NOTICE"));
  copyFileSync(notices, join(dir, "THIRD-PARTY-NOTICES.md"));
  writeFileSync(join(dir, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
}

const optionalDependencies = {};
for (const { target, file } of binaries) {
  const [os, cpu] = PLATFORMS[target];
  const name = `@cori-do/cli-${os}-${cpu}`;
  const dir = join(out, `cli-${os}-${cpu}`);
  mkdirSync(join(dir, "bin"), { recursive: true });
  copyFileSync(file, join(dir, "bin", "cori"));
  chmodSync(join(dir, "bin", "cori"), 0o755);
  writeFileSync(join(dir, "README.md"), `# ${name}\n\nThe \`cori\` binary for ${OS_NAMES[os]} on ${cpu} (${target}), which [@cori-do/cli](https://www.npmjs.com/package/@cori-do/cli) runs: install that one.\n`);
  // no `bin`: @cori-do/cli's launcher is the `cori` command. Yarn's PnP keeps a binary package on disk (preferUnplugged)
  finish(dir, { name, version: main.version, description: `The cori binary for ${OS_NAMES[os]} on ${cpu}, run by @cori-do/cli.`, ...shared, os: [os], cpu: [cpu], files: ["bin/cori", "README.md", "LICENSE", "NOTICE", "THIRD-PARTY-NOTICES.md"], preferUnplugged: true });
  optionalDependencies[name] = main.version;
}

const dir = join(out, "cli");
mkdirSync(join(dir, "bin"), { recursive: true });
copyFileSync(join(here, "bin", "cori.js"), join(dir, "bin", "cori.js"));
chmodSync(join(dir, "bin", "cori.js"), 0o755);
copyFileSync(join(here, "README.md"), join(dir, "README.md"));
finish(dir, { ...main, optionalDependencies });
console.log(`${out}: @cori-do/cli ${main.version} with ${Object.keys(optionalDependencies).join(", ")}`);
