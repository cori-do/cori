/**
 * Curated launcher metadata for ready-made workflows.
 *
 * This is intentionally not a registry. Entries are either versioned git refs
 * or immutable workflow folders bundled with Cori Console; both go through
 * the existing preflight and source-snapshot pipeline. Product-only metadata
 * lives here because the manifest schema stays execution-focused.
 */

export type StarterCategory =
  | "all"
  | "essentials"
  | "local"
  | "google_workspace"
  | "developer";

export type StarterEffect = "read_only" | "writes" | "sends";

export interface StarterWorkflow {
  id: string;
  name: string;
  description: string;
  source: string;
  category: Exclude<StarterCategory, "all">;
  tags: readonly string[];
  tools: readonly string[];
  effect: StarterEffect;
  featured?: boolean;
  platforms?: readonly ("macOS" | "Linux" | "Windows")[];
  google_services?: readonly string[];
  min_tool_version?: string;
  requires_llm?: boolean;
}

export const STARTER_CATEGORIES: ReadonlyArray<{
  id: StarterCategory;
  label: string;
  shortLabel: string;
  description: string;
}> = [
  {
    id: "all",
    label: "Starter library",
    shortLabel: "All starters",
    description: "A curated set of useful workflows you can inspect and run.",
  },
  {
    id: "essentials",
    label: "Cori essentials",
    shortLabel: "Essentials",
    description: "Credential-free examples for learning the Cori workflow model.",
  },
  {
    id: "local",
    label: "Local computer",
    shortLabel: "Local",
    description: "Read-only reports for files, storage, and this computer.",
  },
  {
    id: "google_workspace",
    label: "Google Workspace",
    shortLabel: "Workspace",
    description: "Productivity workflows for Calendar, Gmail, Drive, Docs, and Sheets.",
  },
  {
    id: "developer",
    label: "Developer productivity",
    shortLabel: "Developer",
    description: "Release and repository workflows for engineering teams.",
  },
] as const;

const OFFICIAL_WORKFLOWS = "github.com/cori-do/workflows";
const CORI_REPO = "github.com/cori-do/cori";
const BUNDLED_STARTER = "cori-starter://";

export const STARTER_WORKFLOWS: readonly StarterWorkflow[] = [
  {
    id: "code_only",
    name: "Code-only quick start",
    description: "Square a number and format the result with two pure code steps.",
    source: `${CORI_REPO}/examples/code_only@main`,
    category: "essentials",
    tags: ["demo", "code", "no credentials"],
    tools: [],
    effect: "read_only",
  },
  {
    id: "hn_digest",
    name: "Hacker News digest",
    description: "Build a compact Markdown digest from the current top stories.",
    source: `${OFFICIAL_WORKFLOWS}/hn_digest@v0.2.1`,
    category: "essentials",
    tags: ["digest", "public API", "no credentials"],
    tools: ["curl"],
    effect: "read_only",
    featured: true,
  },
  {
    id: "disk_space_snapshot",
    name: "Disk space snapshot",
    description: "See used and available space for any path on this computer.",
    source: `${BUNDLED_STARTER}disk_space_snapshot`,
    category: "local",
    tags: ["disk", "storage", "computer care"],
    tools: ["df"],
    effect: "read_only",
    platforms: ["macOS", "Linux"],
    featured: true,
  },
  {
    id: "largest_files_report",
    name: "Largest files report",
    description: "Find the largest files above a chosen size without changing them.",
    source: `${BUNDLED_STARTER}largest_files_report`,
    category: "local",
    tags: ["files", "storage", "cleanup planning"],
    tools: ["python3"],
    effect: "read_only",
    platforms: ["macOS", "Linux", "Windows"],
  },
  {
    id: "gws_meeting_prep",
    name: "Next meeting prep",
    description: "Collect the next event, attendees, agenda, and linked documents.",
    source: `${BUNDLED_STARTER}gws_meeting_prep`,
    category: "google_workspace",
    tags: ["calendar", "meetings", "agenda"],
    tools: ["gws"],
    effect: "read_only",
    google_services: ["calendar"],
    min_tool_version: "0.22.5",
    featured: true,
  },
  {
    id: "gws_weekly_digest",
    name: "Workspace weekly digest",
    description: "Summarize this week's meetings and unread-email workload.",
    source: `${BUNDLED_STARTER}gws_weekly_digest`,
    category: "google_workspace",
    tags: ["calendar", "gmail", "weekly review"],
    tools: ["gws"],
    effect: "read_only",
    google_services: ["calendar", "gmail"],
    min_tool_version: "0.22.5",
    featured: true,
  },
  {
    id: "gws_sheet_range_snapshot",
    name: "Sheet range snapshot",
    description: "Read a bounded Google Sheets range as structured rows.",
    source: `${BUNDLED_STARTER}gws_sheet_range_snapshot`,
    category: "google_workspace",
    tags: ["sheets", "data", "snapshot"],
    tools: ["gws"],
    effect: "read_only",
    google_services: ["sheets"],
    min_tool_version: "0.22.5",
  },
  {
    id: "gcal_daily_brief",
    name: "Calendar daily brief",
    description: "Turn upcoming events into an agenda email with one focused AI step.",
    source: `${OFFICIAL_WORKFLOWS}/gcal_daily_brief@v0.2.1`,
    category: "google_workspace",
    tags: ["calendar", "gmail", "daily brief"],
    tools: ["gws"],
    effect: "sends",
    google_services: ["calendar", "gmail"],
    requires_llm: true,
  },
  {
    id: "drive_doc_summarizer",
    name: "Drive document summarizer",
    description: "Summarize a Drive file, create a Google Doc, share it, and email the link.",
    source: `${OFFICIAL_WORKFLOWS}/drive_doc_summarizer@v0.2.1`,
    category: "google_workspace",
    tags: ["drive", "docs", "gmail", "summary"],
    tools: ["gws"],
    effect: "sends",
    google_services: ["drive", "docs", "gmail"],
    requires_llm: true,
  },
  {
    id: "github_release_notes",
    name: "GitHub release notes",
    description: "Draft polished release notes from merged pull requests.",
    source: `${OFFICIAL_WORKFLOWS}/github_release_notes@v0.2.1`,
    category: "developer",
    tags: ["github", "release", "changelog"],
    tools: ["gh"],
    effect: "read_only",
    requires_llm: true,
  },
] as const;

export function starterCategory(category: StarterCategory) {
  return (
    STARTER_CATEGORIES.find((candidate) => candidate.id === category) ??
    STARTER_CATEGORIES[0]!
  );
}

export function startersInCategory(
  category: StarterCategory,
): readonly StarterWorkflow[] {
  if (category === "all") return STARTER_WORKFLOWS;
  return STARTER_WORKFLOWS.filter((workflow) => workflow.category === category);
}

export function starterEffectLabel(effect: StarterEffect): string {
  switch (effect) {
    case "read_only":
      return "Read only";
    case "writes":
      return "Creates or updates";
    case "sends":
      return "Creates and sends";
  }
}
