import { useState } from "react";
import { ChevronRight, Plug } from "lucide-react";
import { api } from "../../lib/api";
import { href } from "../../lib/router";
import { ago } from "../../lib/format";
import { useQuery } from "../../lib/query";
import { useSession } from "../../lib/session";
import type { Connector, ConnectorRequest } from "../../lib/types";
import { Badge, CopyField, cx, Empty, ErrorNote, List, Section, SkeletonRows, Time } from "../../ui/core";
import { PersonName } from "../../ui/people";

// The tools and APIs the organization declared for its apps (MCP servers, HTTP APIs, command-line tools), and what
// builders asked for. They are declared with the command line, where their credentials are given.

export function Connectors() {
  const { me } = useSession();
  const q = useQuery<Connector[]>("/v1/connectors", () => api<Connector[]>("/v1/connectors"));
  const reqs = useQuery<ConnectorRequest[]>(me?.can?.operate ? "/v1/connector-requests" : null, () => api<ConnectorRequest[]>("/v1/connector-requests").catch(() => []));
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={3} />;
  return (
    <>
      {q.data.length ? (
        <List>{q.data.map((c) => <ConnectorRow key={c.name} c={c} />)}</List>
      ) : (
        <Empty icon={<Plug />} title="No connector yet" action={<CopyField value="cori connectors add https://…/mcp --name …" className="w-80 max-w-full" />}>
          A connector lets apps call a tool or an API: an MCP server, an HTTP API, a command-line tool. Declare one with the command line.
        </Empty>
      )}
      {q.data.length > 0 && <p className="mt-3 text-[13px] text-fg-3">To add one: <code className="text-fg-2">cori connectors add &lt;url&gt; --name &lt;name&gt;</code>, or <code className="text-fg-2">--from .mcp.json</code>.</p>}

      {(reqs.data || []).length > 0 && (
        <Section title="Asked for by builders" description="Connectors an app needs that the organization does not have yet." className="mt-10">
          <List>
            {(reqs.data || []).map((r, i) => (
              <div key={i} className="space-y-2 px-4 py-3">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <code className="min-w-0 flex-1 truncate font-medium text-fg">{r.url}</code>
                  <span className="text-[13px] text-fg-3"><PersonName id={r.requested_by} /> · <Time at={r.at}>{ago(r.at)}</Time></span>
                </div>
                {r.reason && <p className="text-[13px] text-fg-2">“{r.reason}”</p>}
                <CopyField value={`cori connectors add ${r.url} --name …`} />
              </div>
            ))}
          </List>
        </Section>
      )}
    </>
  );
}

function ConnectorRow({ c }: { c: Connector }) {
  const [open, setOpen] = useState(false);
  const tools = c.tools || [];
  const where = c.transport.url || (c.transport.command || []).join(" ");
  return (
    <div>
      <button type="button" onClick={() => setOpen(!open)} aria-expanded={open} className="flex w-full cursor-pointer flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 text-left hover:bg-surface-2/60">
        <ChevronRight className={cx("size-4 shrink-0 text-fg-4 transition-transform", open && "rotate-90")} aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-fg">{c.name}</span><Badge>{c.kind}</Badge>
            {c.oauth && <Badge tone="brand" title={c.oauth.scopes.join("\n")}>Each person's {c.oauth.label} account</Badge>}
          </div>
          <div className="truncate text-[13px] text-fg-3">{c.description || where}</div>
        </div>
        <span className="text-[13px] text-fg-3">{tools.length ? `${tools.length} tool${tools.length > 1 ? "s" : ""}` : ""}</span>
        <span className="w-40 truncate text-right text-[13px] text-fg-3">{(c.roles || []).length ? `For ${(c.roles || []).join(", ")}` : "For every app"}</span>
      </button>
      {open && (
        <div className="space-y-2 px-4 pb-4 pl-12 text-[13.5px] text-fg-2">
          {where && <div><span className="text-fg-3">Reached at </span><code className="break-all">{where}</code></div>}
          {tools.length > 0 && <div className="flex flex-wrap gap-1.5">{tools.map((t) => <code key={t.name} className="rounded-md bg-surface-2 px-1.5 py-0.5 text-[12.5px] ring-1 ring-line ring-inset">{t.name}</code>)}</div>}
          {c.provided === "deployment"
            ? <div className="text-[13px] text-fg-3">Provided by this server, for every organization{c.oauth && <>: each run acts with its person's {c.oauth.label} account (<a href={href("connections")} className="link">yours</a>), or the app's</>}.</div>
            : c.declared_by && <div className="text-[13px] text-fg-3">Declared by <PersonName id={c.declared_by} /> <Time at={c.declared_at}>{ago(c.declared_at)}</Time></div>}
        </div>
      )}
    </div>
  );
}
