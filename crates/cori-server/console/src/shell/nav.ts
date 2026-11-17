import type { Me, MyApp } from "../lib/types";
import { href } from "../lib/router";

// What the person can open, shared by the sidebar, an app's tabs and the command palette. Nothing is labelled by persona:
// a tab or a section shows when the person's roles reach it.

export type AppTab = { key: string; label: string; href: string };

export function appTabs(app: string, mine: MyApp | null, opts: { admin?: boolean } = {}): AppTab[] {
  const builds = !!mine?.build || !!opts.admin;
  const tabs: [string, string, boolean][] = [
    ["overview", "Overview", true],
    ["releases", "Releases", builds],
    ["runs", builds ? "Runs" : "My runs", true],
    ["data", "Data", builds || !!mine?.stores.length],
    ["access", "Access", builds],
    ["settings", "Settings", builds],
  ];
  return tabs.filter(([, , on]) => on).map(([key, label]) => ({ key, label, href: key === "overview" ? href("app", app) : href("app", app, key) }));
}

export type SettingsSection = { key: string; label: string; description: string };

export function settingsSections(me?: Me, builds = false): SettingsSection[] {
  // who reaches their apps only (a guest, a person with no power who builds no app) has nothing to set
  if (me?.apps_only || me?.guest) return [];
  const can = me?.can ?? {};
  const member = !!can.people;
  const all: [string, string, string, boolean][] = [
    ["general", "General", "Name, address and export of the organization", member],
    ["members", "Members", "Who belongs to the organization, and their roles", member],
    ["models", "AI models", "Model providers, default models and monthly budgets", !!(can.operate || can.usage || can.admin)],
    ["connectors", "Connectors", "The tools and APIs your apps may call", builds || !!can.operate],
    ["credentials", "Credentials", "Secrets your apps and connectors use", !!(can.operate || can.admin)],
    ["system", "System", "Health, sign-in sync, databases and workers", !!can.operate],
    ["audit", "Audit log", "Who did what, and when", !!can.audit],
  ];
  return all.filter(([, , , on]) => on).map(([key, label, description]) => ({ key, label, description }));
}
