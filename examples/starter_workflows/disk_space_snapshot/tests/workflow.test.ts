import workflow from "../steps/01_inspect_disk_space.ts";

function assertEquals(actual: unknown, expected: unknown) {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, received ${actualJson}`);
  }
}

Deno.test("builds and parses a POSIX df snapshot", async () => {
  assertEquals(workflow.command({ path: "/" }), ["df", "-Pk", "/"]);
  const output = await workflow.parse!(
    "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk1 1000 250 750 25% /\n",
    { stderr: "", exitCode: 0 },
  );
  assertEquals(output, {
    results: {
      title: "Disk space snapshot",
      summary: "/ is 25% full.",
      filesystems: [{
        filesystem: "/dev/disk1",
        mount_point: "/",
        total_bytes: 1024000,
        used_bytes: 256000,
        available_bytes: 768000,
        used_percent: 25,
      }],
    },
  });
});
