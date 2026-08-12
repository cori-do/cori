import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  path: z.string().min(1),
  limit: z.number().int().min(1).max(1000),
  minimum_size_mb: z.number().nonnegative(),
});

const FileEntry = z.object({
  path: z.string().min(1),
  size_bytes: z.number().int().nonnegative(),
  size_mb: z.number().nonnegative(),
});

const RawOutput = z.object({
  scanned_root: z.string().min(1),
  files_scanned: z.number().int().nonnegative(),
  skipped_entries: z.number().int().nonnegative(),
  total_bytes_scanned: z.number().int().nonnegative(),
  minimum_size_bytes: z.number().int().nonnegative(),
  largest_files: z.array(FileEntry),
});

const Output = z.object({
  results: z.object({
    title: z.string(),
    summary: z.string(),
    scanned_root: z.string().min(1),
    files_scanned: z.number().int().nonnegative(),
    skipped_entries: z.number().int().nonnegative(),
    total_bytes_scanned: z.number().int().nonnegative(),
    minimum_size_bytes: z.number().int().nonnegative(),
    files: z.array(FileEntry),
  }),
});

const PYTHON_PROGRAM = [
  "import heapq, json, os, stat, sys",
  "root = os.path.abspath(os.path.expanduser(sys.argv[1]))",
  "limit = int(sys.argv[2])",
  "minimum_bytes = int(float(sys.argv[3]) * 1024 * 1024)",
  "if not os.path.isdir(root):",
  "    raise SystemExit(f'not a readable directory: {root}')",
  "heap = []",
  "files_scanned = 0",
  "skipped_entries = 0",
  "total_bytes_scanned = 0",
  "walk_errors = []",
  "for directory, dirnames, filenames in os.walk(root, followlinks=False, onerror=walk_errors.append):",
  "    dirnames.sort()",
  "    filenames.sort()",
  "    for filename in filenames:",
  "        full_path = os.path.join(directory, filename)",
  "        try:",
  "            metadata = os.stat(full_path, follow_symlinks=False)",
  "        except OSError:",
  "            skipped_entries += 1",
  "            continue",
  "        if not stat.S_ISREG(metadata.st_mode):",
  "            continue",
  "        size = metadata.st_size",
  "        files_scanned += 1",
  "        total_bytes_scanned += size",
  "        if size < minimum_bytes:",
  "            continue",
  "        candidate = (size, full_path)",
  "        if len(heap) < limit:",
  "            heapq.heappush(heap, candidate)",
  "        elif candidate > heap[0]:",
  "            heapq.heapreplace(heap, candidate)",
  "skipped_entries += len(walk_errors)",
  "largest = sorted(heap, key=lambda item: (-item[0], item[1]))",
  "output = {",
  "    'scanned_root': root,",
  "    'files_scanned': files_scanned,",
  "    'skipped_entries': skipped_entries,",
  "    'total_bytes_scanned': total_bytes_scanned,",
  "    'minimum_size_bytes': minimum_bytes,",
  "    'largest_files': [",
  "        {'path': path, 'size_bytes': size, 'size_mb': round(size / (1024 * 1024), 2)}",
  "        for size, path in largest",
  "    ],",
  "}",
  "print(json.dumps(output, sort_keys=True))",
].join("\n");

export default step.cli({
  description: "Find the largest regular files below a folder",
  input: Input,
  output: Output,
  command: ({ path, limit, minimum_size_mb }) => [
    "python3",
    "-c",
    PYTHON_PROGRAM,
    path,
    String(limit),
    String(minimum_size_mb),
  ],
  parse: (stdout) => {
    const raw = RawOutput.parse(JSON.parse(stdout));
    const count = raw.largest_files.length;
    return Output.parse({
      results: {
        title: "Largest files report",
        summary: `Found ${count} ${
          count === 1 ? "file" : "files"
        } above the selected size threshold.`,
        scanned_root: raw.scanned_root,
        files_scanned: raw.files_scanned,
        skipped_entries: raw.skipped_entries,
        total_bytes_scanned: raw.total_bytes_scanned,
        minimum_size_bytes: raw.minimum_size_bytes,
        files: raw.largest_files,
      },
    });
  },
});
