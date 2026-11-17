import { useState, type ReactNode } from "react";
import { ExternalLink, GitBranch, MoreHorizontal, Rocket } from "lucide-react";
import { api } from "../../lib/api";
import { ago, plural, when } from "../../lib/format";
import { personName } from "../../lib/people";
import { useQuery } from "../../lib/query";
import { href, navigate } from "../../lib/router";
import type { AppDetail, Deployment, Version, VersionDetail } from "../../lib/types";
import { Button, ButtonLink, Callout, copy, CopyField, cx, Empty, ErrorNote, IconButton, List, Section, SkeletonRows, Time } from "../../ui/core";
import { Drawer } from "../../ui/dialog";
import { Menu } from "../../ui/menu";
import { PersonName } from "../../ui/people";
import { Json } from "../../ui/value";
import { Changes } from "../Inbox";
import { deleteRelease, GateBadge, goLive, goLiveLabel, resetPreview } from "./releases-lib";

// Releases: the ones ready to go live, the live one, the earlier ones. Each new release runs on its own preview data at its
// own address; one button makes it what people use. `#/app/<app>/releases/<release>` opens one's details.

const EARLIER_SHOWN = 5;

export function Releases({ app, d, open }: { app: string; d?: AppDetail; open?: string }) {
  const q = useQuery<Version[]>(`/v1/apps/${app}/versions`, () => api<Version[]>(`/v1/apps/${app}/versions`));
  const history = useQuery<Deployment[]>(`/v1/apps/${app}/promotions`, () => api<Deployment[]>(`/v1/apps/${app}/promotions`).catch(() => []));
  const [all, setAll] = useState(false);
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={3} />;
  const releases = q.data;
  const live = releases.find((r) => r.section === "live");
  const newer = releases.filter((r) => r.section === "newer");
  const older = releases.filter((r) => r.section === "older");
  // one accent button on the page: the newest release that can go live
  const primary = newer.find((r) => r.go_live === "now" || r.go_live === "approve")?.hash;
  const selected = open ? releases.find((r) => r.release === open || r.short === open || r.hash === open) : undefined;
  const openOne = (r: Version) => navigate(href("app", app, "releases", r.release));
  const row = (r: Version) => <ReleaseRow key={r.hash} app={app} r={r} live={live} frontUrl={d?.front_url} primary={r.hash === primary} onOpen={() => openOne(r)} />;

  return (
    <>
      <p className="-mt-1 mb-1 text-[13px] text-fg-3">
        Going live needs an approver {d?.protected ? "for every new release" : "when a release adds or changes a write"}. Rolling back to a release that was live never does.{" "}
        {d?.build !== false && <a href={href("app", app, "settings")} className="link">Change</a>}
      </p>

      {releases.length === 0 && (
        <Empty className="mt-6" icon={<Rocket />} title="No release yet" action={<CopyField value="cori deploy" className="w-64" />}>
          Ask your agent to deploy the app, or run this in its folder. Each release gets its own preview and test data; nothing reaches people until you make it live.
        </Empty>
      )}

      {newer.length > 0 && <Section title="Ready to go live">{<List>{newer.map(row)}</List>}</Section>}
      {live && <Section title="Live">{<List>{row(live)}</List>}</Section>}
      {older.length > 0 && (
        <Section title="Earlier releases" actions={older.length > EARLIER_SHOWN && <button type="button" onClick={() => setAll(!all)} className="cursor-pointer text-[13px] text-fg-3 hover:text-fg">{all ? "Show fewer" : `Show all ${older.length}`}</button>}>
          <List>{(all ? older : older.slice(0, EARLIER_SHOWN)).map(row)}</List>
        </Section>
      )}

      {(history.data || []).length > 0 && (
        <Section title="Live history" description="What people used, and when it changed.">
          <ol className="relative ml-1.5 space-y-3 border-l border-line pl-5">
            {(history.data || []).slice(0, 8).map((x, i) => {
              const r = releases.find((v) => v.hash === x.hash);
              return (
                <li key={i} className="relative text-[14px]">
                  <span className={cx("absolute top-1.5 -left-[25px] size-2.5 rounded-full ring-4 ring-canvas", i === 0 ? "bg-emerald-500" : "bg-fg-4")} />
                  <span className="font-medium text-fg">{r?.release || "A deleted release"}</span>{" "}
                  <span className="text-fg-3">{x.kind === "rollback" ? "rolled back to" : "went live"} · <PersonName id={x.by} /> · <Time at={x.at}>{when(x.at)}</Time>
                    {(x.migrations || []).length ? ` · ${plural(x.migrations!.length, "migration")} on live data` : ""}</span>
                </li>
              );
            })}
          </ol>
        </Section>
      )}

      {open && (selected
        ? <ReleaseDrawer key={selected.hash} app={app} r={selected} live={live} frontUrl={d?.front_url} onClose={() => navigate(href("app", app, "releases"))} />
        : !q.loading && <Drawer title={open} onClose={() => navigate(href("app", app, "releases"))}><Empty compact title="No such release">It may have been deleted.</Empty></Drawer>)}
    </>
  );
}

function facts(r: Version) {
  return [
    `${personName(r.created_by)} · ${ago(r.created_at)}`,
    r.git_commit ? `${r.git_branch || "?"}@${r.git_commit.slice(0, 7)}${r.git_dirty ? "*" : ""}` : null,
    r.pending_migrations.length ? plural(r.pending_migrations.length, "migration") : null,
  ].filter(Boolean).join(" · ");
}

function ReleaseRow({ app, r, live, frontUrl, primary, onOpen }: { app: string; r: Version; live?: Version; frontUrl?: string | null; primary: boolean; onOpen: () => void }) {
  const label = goLiveLabel(r);
  return (
    <div className="group relative flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 transition-colors hover:bg-surface-2/60 sm:flex-nowrap">
      <span className={cx("w-10 shrink-0 font-semibold tabular-nums", r.section === "older" ? "text-fg-3" : "text-fg")}>{r.release}</span>
      <button type="button" onClick={onOpen} className="min-w-0 flex-1 cursor-pointer text-left after:absolute after:inset-0">
        <span className="block truncate text-fg">{r.message || <span className="text-fg-3">No message</span>}</span>
        <span className="block truncate text-[13px] text-fg-3">{facts(r)}</span>
      </button>
      <div className="relative z-10 flex shrink-0 items-center gap-2">
        <GateBadge r={r} />
        {r.live
          ? frontUrl && <ButtonLink href={frontUrl} external size="xs" variant="ghost" icon={<ExternalLink />}>Open</ButtonLink>
          : <ButtonLink href={r.preview.url} external size="xs" variant="ghost" icon={<ExternalLink />}>Preview</ButtonLink>}
        {label && <Button size="xs" variant={primary ? "primary" : "secondary"} onClick={() => goLive(app, r, live, frontUrl)}>{label}</Button>}
        <RowMenu app={app} r={r} onOpen={onOpen} />
      </div>
    </div>
  );
}

function RowMenu({ app, r, onOpen }: { app: string; r: Version; onOpen: () => void }) {
  return (
    <Menu items={[
      { label: "Details", onSelect: onOpen },
      r.preview.stores.length > 0 && !r.live && { label: "Reset preview data", onSelect: () => resetPreview(app, r) },
      { label: "Copy pull command", onSelect: () => copy(`cori pull ${app} ./${app} --hash ${r.release}`, "Pull command copied") },
      !r.live && "sep",
      !r.live && { label: "Delete release", danger: true, onSelect: () => deleteRelease(app, r) },
    ]}>
      {(p) => <IconButton {...p} label={`More actions for ${r.release}`} size="xs"><MoreHorizontal /></IconButton>}
    </Menu>
  );
}

const EVENT: Record<string, string> = { deployed: "Deployed", redeployed: "Deployed again (same content)", approved: "Approved", go_live_requested: "Go-live requested", went_live: "Went live", replaced: "Replaced by another release", preview_reset: "Preview data reset" };

function ReleaseDrawer({ app, r, live, frontUrl, onClose }: { app: string; r: Version; live?: Version; frontUrl?: string | null; onClose: () => void }) {
  const q = useQuery<VersionDetail>(`/v1/apps/${app}/versions/${r.hash}`, () => api<VersionDetail>(`/v1/apps/${app}/versions/${r.hash}`));
  const [raw, setRaw] = useState(false);
  const label = goLiveLabel(r);
  const delta = { ...(r.delta || {}), migrations: r.pending_migrations };
  return (
    <Drawer title={<span className="flex items-center gap-2.5">{r.release}<GateBadge r={r} /></span>} subtitle={r.message || "No message"} onClose={onClose}
      footer={<>
        {!r.live && <Button variant="danger-soft" className="mr-auto" onClick={async () => { if (await deleteRelease(app, r)) onClose(); }}>Delete</Button>}
        {!r.live ? <ButtonLink href={r.preview.url} external icon={<ExternalLink />}>Preview</ButtonLink>
          : frontUrl && <ButtonLink href={frontUrl} external icon={<ExternalLink />}>Open app</ButtonLink>}
        {label && <Button variant="primary" onClick={async () => { if (await goLive(app, r, live, frontUrl)) onClose(); }}>{label}</Button>}
      </>}>
      <div className="space-y-7">
        <Part title={r.live ? "Live" : live ? `If it goes live, compared with ${live.release}` : "If it goes live"}>
          {r.live ? <p className="text-fg-2">This is what people use{frontUrl ? <> at <a href={frontUrl} target="_blank" rel="noreferrer" className="link">{frontUrl.replace(/^https?:\/\//, "")}</a></> : null}.</p>
            : r.was_live ? <p className="text-fg-2">It was live before: rolling back to it needs no approval and runs no migration.</p>
            : <Changes delta={delta} />}
          {r.schema_ahead.length > 0 && <Callout tone="warning" className="mt-3">The live data ran migrations this release does not have: <code>{r.schema_ahead.join(", ")}</code>. Its code runs against that newer schema.</Callout>}
          {!r.live && r.gate.state === "needs_approval" && <Callout className="mt-3" title="An approver decides it">{r.gate.reason}</Callout>}
          {!r.live && r.gate.state === "requested" && <Callout className="mt-3" title={`${personName(r.gate.by)} asked for it to go live`}>An approver decides it from their inbox{r.gate.reason ? ` (${r.gate.reason})` : ""}.</Callout>}
          {!r.live && r.gate.state === "approved" && <Callout tone="success" className="mt-3" title={`Approved by ${personName(r.gate.by)}`}>{r.gate.against ? `Against ${r.gate.against}: another release going live first lapses the approval.` : "It can go live now."}</Callout>}
        </Part>

        {!r.live && (
          <Part title="Preview">
            <p className="text-fg-2">Try it at <a href={r.preview.url} target="_blank" rel="noreferrer" className="link break-all">{r.preview.url.replace(/^https?:\/\//, "")}</a>, on preview data of its own.</p>
            {r.preview.stores.length > 0 && (
              <p className="mt-2 text-[13px] text-fg-3">Its test data: {r.preview.stores.map((s, i) => <span key={s}>{i ? ", " : ""}<a href={href("app", app, "data", r.short, s)} className="link">{s}</a></span>)}</p>
            )}
          </Part>
        )}

        <Part title="Details">
          <dl className="grid grid-cols-[7rem_1fr] gap-y-2 text-[14px]">
            <dt className="text-fg-3">Deployed</dt><dd><PersonName id={r.created_by} /> · <Time at={r.created_at}>{when(r.created_at)}</Time></dd>
            {r.git_commit && <><dt className="text-fg-3">Source</dt><dd className="flex items-center gap-1.5"><GitBranch className="size-3.5 text-fg-3" />{r.git_branch || "?"} @ <code>{r.git_commit.slice(0, 7)}</code>{r.git_dirty && <span className="text-amber-700 dark:text-amber-300">with uncommitted changes</span>}</dd></>}
            <dt className="text-fg-3">Content</dt><dd><code className="text-[12.5px] text-fg-2">{r.short}</code></dd>
          </dl>
        </Part>

        {q.data && q.data.events.length > 0 && (
          <Part title="History">
            <ol className="space-y-2 text-[14px]">
              {q.data.events.map((e, i) => (
                <li key={i} className="flex gap-3">
                  <span className="w-28 shrink-0 text-[13px] text-fg-3 tabular-nums"><Time at={e.at}>{when(e.at)}</Time></span>
                  <span className="min-w-0">{EVENT[e.kind] || e.kind} <span className="text-fg-3">by <PersonName id={e.by} /></span></span>
                </li>
              ))}
            </ol>
          </Part>
        )}

        {q.data && (
          <div>
            <button type="button" className="cursor-pointer text-[13px] text-fg-3 hover:text-fg" onClick={() => setRaw(!raw)}>{raw ? "Hide" : "Show"} the full change review</button>
            {raw && <Json className="mt-2 max-h-96" value={q.data.review} />}
          </div>
        )}
      </div>
    </Drawer>
  );
}

const Part = ({ title, children }: { title: string; children: ReactNode }) => (
  <section><h3 className="mb-2.5 text-[13px] font-medium text-fg-3">{title}</h3>{children}</section>
);
