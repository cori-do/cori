import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  path: z.string().min(1),
});

const Filesystem = z.object({
  filesystem: z.string().min(1),
  mount_point: z.string().min(1),
  total_bytes: z.number().int().nonnegative(),
  used_bytes: z.number().int().nonnegative(),
  available_bytes: z.number().int().nonnegative(),
  used_percent: z.number().nonnegative(),
});

const Output = z.object({
  results: z.object({
    title: z.string(),
    summary: z.string(),
    filesystems: z.array(Filesystem).min(1),
  }),
});

export default step.cli({
  description: "Report disk space for the selected path",
  input: Input,
  output: Output,
  command: ({ path }) => ["df", "-Pk", path],
  parse: (stdout) => {
    const lines = stdout
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const filesystems = lines.slice(1).map((line) => {
      const columns = line.split(/\s+/);
      if (columns.length < 6) {
        throw new Error(`Unexpected df output row: ${line}`);
      }
      const [filesystem, blocks, used, available, capacity, ...mountParts] =
        columns;
      return {
        filesystem,
        mount_point: mountParts.join(" "),
        total_bytes: Number.parseInt(blocks, 10) * 1024,
        used_bytes: Number.parseInt(used, 10) * 1024,
        available_bytes: Number.parseInt(available, 10) * 1024,
        used_percent: Number.parseFloat(capacity.replace(/%$/, "")),
      };
    });
    const primary = filesystems[0];
    if (!primary) throw new Error("df returned no filesystem rows");
    return Output.parse({
      results: {
        title: "Disk space snapshot",
        summary: `${primary.mount_point} is ${primary.used_percent}% full.`,
        filesystems,
      },
    });
  },
});
