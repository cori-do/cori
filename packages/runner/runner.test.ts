const ENVELOPE_PREFIX = "\u001eCORI_RUNNER\u001e";

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
