import { useEffect } from "react";
import { usePeople } from "../../lib/people";
import { href, navigate } from "../../lib/router";
import { useSession } from "../../lib/session";
import { useTitle } from "../../lib/title";
import { settingsSections } from "../../shell/nav";
import { cx, Empty, Select, Spinner } from "../../ui/core";
import { Audit } from "./Audit";
import { Connectors } from "./Connectors";
import { Credentials } from "./Credentials";
import { General } from "./General";
import { Members } from "./Members";
import { Models } from "./Models";
import { System } from "./System";

// The organization's settings, one section at a time; a section shows when the person's roles reach it.

export function SettingsPage({ section }: { section?: string }) {
  const s = useSession();
  usePeople();
  const sections = settingsSections(s.me, s.builds);
  const current = sections.find((x) => x.key === section);
  useTitle(current?.label, "Settings");
  useEffect(() => { if (s.me && !current && sections.length) navigate(href("settings", sections[0].key), { replace: true }); }, [s.me, current, sections]);
  if (s.loading && !s.me) return <Spinner />;
  if (!sections.length) return <Empty title="Nothing to set here">Your roles reach no setting of the organization.</Empty>;
  if (!current) return null;
  const org = s.me?.organization;
  return (
    <>
      <header className="mb-6">
        <h1 className="text-[22px] leading-tight font-semibold tracking-tight">Settings</h1>
        <p className="mt-1 text-fg-3">{org?.name || org?.slug ? `${org.name || org.slug}: how the organization works.` : "How this server works for everyone."}</p>
      </header>
      <div className="grid items-start gap-6 lg:grid-cols-[200px_minmax(0,1fr)] lg:gap-10">
        <nav aria-label="Settings" className="hidden lg:sticky lg:top-8 lg:block">
          <ul className="space-y-0.5">
            {sections.map((x) => (
              <li key={x.key}>
                <a href={href("settings", x.key)} aria-current={x.key === current.key ? "page" : undefined}
                  className={cx("block rounded-lg px-3 py-1.5 text-[14px] transition-colors", x.key === current.key ? "bg-surface font-medium text-fg shadow-xs ring-1 ring-line" : "text-fg-2 hover:bg-surface-3/60 hover:text-fg")}>
                  {x.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <Select className="lg:hidden" value={current.key} aria-label="Settings section" onChange={(e) => navigate(href("settings", e.target.value))}>
          {sections.map((x) => <option key={x.key} value={x.key}>{x.label}</option>)}
        </Select>
        <div className="min-w-0">
          <div className="mb-6">
            <h2 className="text-lg font-semibold">{current.label}</h2>
            <p className="mt-0.5 text-[13.5px] text-fg-3">{current.description}.</p>
          </div>
          <div>
            {current.key === "general" ? <General />
              : current.key === "members" ? <Members />
              : current.key === "models" ? <Models />
              : current.key === "connectors" ? <Connectors />
              : current.key === "credentials" ? <Credentials />
              : current.key === "system" ? <System />
              : <Audit />}
          </div>
        </div>
      </div>
    </>
  );
}
