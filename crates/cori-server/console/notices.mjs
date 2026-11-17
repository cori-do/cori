// SPDX-License-Identifier: Apache-2.0
// Writes THIRD-PARTY-NOTICES.md: the licences of the JavaScript the built console carries, which the `cori` binary
// embeds (dist/index.html, every licence comment minified away). That is the console's dependencies with theirs, and
// Tailwind CSS, whose generated stylesheet is inlined too. Run by `npm run build`; the cli's release appends the file
// to the Rust crates' notices (scripts/cli-notices.sh).
//   node notices.mjs [<front folder> <out file> <heading>]   another built front the binary embeds (scripts/template-front.sh)
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [dirArg, outArg, heading = "JavaScript in the console"] = process.argv.slice(2);
const root = dirArg ? resolve(dirArg) : dirname(fileURLToPath(import.meta.url));
const out = outArg ? resolve(outArg) : join(root, "THIRD-PARTY-NOTICES.md");
const manifest = (dir) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
/** Where a package resolves from `from` (node's lookup: node_modules up the tree). */
function locate(name, from) {
  for (let dir = from; ; dir = dirname(dir)) {
    const candidate = join(dir, "node_modules", name);
    if (existsSync(join(candidate, "package.json"))) return candidate;
    if (dirname(dir) === dir) throw new Error(`${name} is not installed (npm ci first)`);
  }
}

const seen = new Map();
function visit(name, from) {
  const dir = locate(name, from);
  const pkg = manifest(dir);
  const key = `${pkg.name}@${pkg.version}`;
  if (seen.has(key)) return;
  const file = readdirSync(dir).find((f) => /^licen[cs]e(\.(md|txt))?$/i.test(f));
  seen.set(key, { name: pkg.name, version: pkg.version, license: pkg.license ?? "see its text", text: file ? readFileSync(join(dir, file), "utf8").trim() : undefined });
  for (const dep of Object.keys(pkg.dependencies ?? {})) visit(dep, dir);
}
for (const dep of [...Object.keys(manifest(root).dependencies ?? {}), "tailwindcss"]) visit(dep, root);

const missing = [...seen.values()].filter((p) => !p.text).map((p) => `${p.name}@${p.version}`);
if (missing.length) throw new Error(`no licence text in ${missing.join(", ")}`);
const sections = [...seen.values()].sort((a, b) => a.name.localeCompare(b.name))
  .map((p) => `### ${p.name} ${p.version} (${p.license})\n\n\`\`\`\n${p.text}\n\`\`\`\n`);
writeFileSync(out, `## ${heading}\n\n${sections.join("\n")}`);
console.log(`${out}: ${[...seen.keys()].join(", ")}`);
