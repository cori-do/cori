const ENVELOPE_PREFIX = "\u001eCORI_RUNNER\u001e";

async function invokeRunner(
  temporary: string,
  stepPath: string,
  mode: string,
  payload: unknown,
): Promise<{ ok?: unknown; output?: unknown; error?: { message?: string } }> {
  const runnerPath = new URL("./runner.ts", import.meta.url).pathname;
  const configPath = new URL("./deno.json", import.meta.url).pathname;
  const lockPath = new URL("./deno.lock", import.meta.url).pathname;
  const command = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--quiet",
      "--no-prompt",
      "--cached-only",
      "--no-remote",
      "--config",
      configPath,
      "--lock",
      lockPath,
      "--frozen",
      `--allow-read=${temporary},${new URL(".", import.meta.url).pathname}`,
      runnerPath,
      stepPath,
      mode,
    ],
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  });
  const child = command.spawn();
  const writer = child.stdin.getWriter();
  await writer.write(new TextEncoder().encode(JSON.stringify(payload)));
  await writer.close();
  const output = await child.output();
  const stdout = new TextDecoder().decode(output.stdout);
  const envelope = stdout.split(/\r?\n/u).find((line) =>
    line.startsWith(ENVELOPE_PREFIX)
  );
  if (!envelope) {
    const stderr = new TextDecoder().decode(output.stderr);
    throw new Error(`runner emitted no envelope: ${stdout}\n${stderr}`);
  }
  return JSON.parse(envelope.slice(ENVELOPE_PREFIX.length));
}

const BRANCH_STEP_SOURCE = [
  "export default {",
  "  __cori_step: true,",
  '  kind: "builtin",',
  '  builtin: "branch",',
  "  if: ({ count }) => count > 10,",
  "  then: {",
  "    __cori_step: true,",
  '    kind: "code",',
  '    run: ({ count }) => ({ verdict: "big", count }),',
  "  },",
  "  else: {",
  "    __cori_step: true,",
  '    kind: "code",',
  '    run: ({ count }) => ({ verdict: "small", count }),',
  "  },",
  "};",
  "",
].join("\n");

Deno.test("runner builtin_eval evaluates a branch's `if` selector", async () => {
  const temporary = await Deno.makeTempDir({ prefix: "cori-runner-builtin-" });
  try {
    const stepPath = `${temporary}/03_branch.ts`;
    await Deno.writeTextFile(stepPath, BRANCH_STEP_SOURCE);

    const truthy = await invokeRunner(temporary, stepPath, "builtin_eval", {
      input: { count: 42 },
      eval: "if",
    });
    if (truthy.ok !== true || truthy.output !== true) {
      throw new Error(`unexpected envelope: ${JSON.stringify(truthy)}`);
    }
    const falsy = await invokeRunner(temporary, stepPath, "builtin_eval", {
      input: { count: 2 },
      eval: "if",
    });
    if (falsy.ok !== true || falsy.output !== false) {
      throw new Error(`unexpected envelope: ${JSON.stringify(falsy)}`);
    }
  } finally {
    await Deno.remove(temporary, { recursive: true });
  }
});

Deno.test("runner selector runs a nested step through an existing mode", async () => {
  const temporary = await Deno.makeTempDir({ prefix: "cori-runner-selector-" });
  try {
    const stepPath = `${temporary}/03_branch.ts`;
    await Deno.writeTextFile(stepPath, BRANCH_STEP_SOURCE);

    const nested = await invokeRunner(temporary, stepPath, "code", {
      input: { count: 42 },
      selector: "then",
    });
    if (
      nested.ok !== true ||
      JSON.stringify(nested.output) !==
        JSON.stringify({ verdict: "big", count: 42 })
    ) {
      throw new Error(`unexpected envelope: ${JSON.stringify(nested)}`);
    }

    const missing = await invokeRunner(temporary, stepPath, "code", {
      input: { count: 1 },
      selector: "cases.nope",
    });
    if (
      missing.ok !== false ||
      !String(missing.error?.message ?? "").includes("does not resolve")
    ) {
      throw new Error(`unexpected envelope: ${JSON.stringify(missing)}`);
    }
  } finally {
    await Deno.remove(temporary, { recursive: true });
  }
});

Deno.test("runner code mode emits a successful protocol envelope", async () => {
  const temporary = await Deno.makeTempDir({ prefix: "cori-runner-smoke-" });
  try {
    const stepPath = `${temporary}/01_double.ts`;
    await Deno.writeTextFile(
      stepPath,
      [
        "export default {",
        "  __cori_step: true,",
        '  kind: "code",',
        "  run: ({ value }) => ({ doubled: value * 2 }),",
        "};",
        "",
      ].join("\n"),
    );

    const runnerPath = new URL("./runner.ts", import.meta.url).pathname;
    const configPath = new URL("./deno.json", import.meta.url).pathname;
    const lockPath = new URL("./deno.lock", import.meta.url).pathname;
    const command = new Deno.Command(Deno.execPath(), {
      args: [
        "run",
        "--quiet",
        "--no-prompt",
        "--cached-only",
        "--no-remote",
        "--config",
        configPath,
        "--lock",
        lockPath,
        "--frozen",
        `--allow-read=${temporary},${new URL(".", import.meta.url).pathname}`,
        runnerPath,
        stepPath,
        "code",
      ],
      stdin: "piped",
      stdout: "piped",
      stderr: "piped",
    });
    const child = command.spawn();
    const writer = child.stdin.getWriter();
    await writer.write(
      new TextEncoder().encode(JSON.stringify({ input: { value: 21 } })),
    );
    await writer.close();
    const output = await child.output();
    const stdout = new TextDecoder().decode(output.stdout);
    const stderr = new TextDecoder().decode(output.stderr);

    if (!output.success) {
      throw new Error(`runner smoke failed: ${stderr || stdout}`);
    }
    const envelope = stdout.split(/\r?\n/u).find((line) =>
      line.startsWith(ENVELOPE_PREFIX)
    );
    if (!envelope) throw new Error(`runner emitted no envelope: ${stdout}`);
    const parsed = JSON.parse(envelope.slice(ENVELOPE_PREFIX.length)) as {
      ok?: unknown;
      output?: unknown;
    };
    if (
      parsed.ok !== true ||
      JSON.stringify(parsed.output) !== JSON.stringify({ doubled: 42 })
    ) {
      throw new Error(`unexpected runner envelope: ${envelope}`);
    }
  } finally {
    await Deno.remove(temporary, { recursive: true });
  }
});
