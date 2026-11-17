import { useState, type ReactNode } from "react";
import { Bot, ExternalLink } from "lucide-react";
import { api, authConfig, claudeMcpAdd, del, errMsg } from "../lib/api";
import { brand } from "../lib/brand";
import { ago, day } from "../lib/format";
import { useQuery } from "../lib/query";
import { href } from "../lib/router";
import { useSession } from "../lib/session";
import { useTitle } from "../lib/title";
import type { AgentSession, StartState } from "../lib/types";
import { Badge, Button, ButtonLink, Card, CopyField, cx, Empty, List, PageHeader, Section, Segmented, SkeletonRows, Time } from "../ui/core";
import { confirm } from "../ui/dialog";
import { PersonName } from "../ui/people";
import { toast } from "../ui/toast";

// Building happens in the person's coding agent, which signs in to this server through the browser the first time and
// gets the command line's session from the MCP server: nothing here holds a secret. `#/connect/new` leads with creating
// an app; the agents they connected are listed, and can be revoked.

type Host = "claude-code" | "claude" | "cursor" | "vscode" | "other";

export function Connect({ focus }: { focus?: string }) {
  const creating = focus === "new";
  useTitle(creating ? "Create an app" : "Connect your agent");
  const { me } = useSession();
  const start = useQuery<StartState>("/v1/start", () => api<StartState>("/v1/start"));
  const org = me?.organization;
  const slug = org?.slug || "";
  return (
    <>
      <PageHeader title={creating ? "Create an app" : "Connect your agent"}
        description={creating
          ? `Apps are built by your coding agent, from a description in your own words. ${brand.name} gives them a backend, data, approvals and a sign-in; you try each release, then make it live here.`
          : `Your coding agent builds, checks and deploys your apps. You try them, approve and go live here.`} />

      {creating && (
        <Section title="Describe it to your agent" description={start.data && !start.data.agent.done ? "Connect your agent first (below), then ask it:" : "Ask it, in your own words:"}>
          <Card className="space-y-4 p-5">
            <CopyField multiline value={`Build me an internal app on ${brand.name} for … Then deploy it and give me its preview link.`} />
            <p className="text-[13px] text-fg-3">
              Your agent creates the app, deploys a first release on its own preview data, and gives you its link. The app then shows in your sidebar: open it to try the release and make it live.
            </p>
            <div className="border-t border-line pt-4">
              <p className="text-[13px] font-medium text-fg">Or start from a finished example</p>
              <p className="mt-0.5 mb-2 text-[13px] text-fg-3">Team requests: a form, an AI triage step, a manager's approval and its screens.</p>
              <CopyField value="cori init my-app --template team-requests && cd my-app && cori deploy" />
            </div>
          </Card>
        </Section>
      )}

      <Section title={creating ? "Connect your agent" : "Add it to your agent"} description={start.data?.agent.done ? <>Connected{start.data.agent.detail ? <> with {start.data.agent.detail}</> : null}{start.data.agent.by ? <> by <PersonName id={start.data.agent.by} /></> : null}.</> : "The first time, it opens your browser for you to sign in and allow it."}>
        <Agents />
        {org && (
          <p className="mt-3 text-[13px] text-fg-3">
            Working in several organizations? Add this server once per organization, under its own name{slug && <>, for example <code className="text-fg-2">cori-{slug}</code></>}: an agent works in the organization it signed in to.
          </p>
        )}
      </Section>

      <Section title="The command line" description="Your agent uses it to check and deploy. To use it yourself, sign in once.">
        <Card className="space-y-3 p-5">
          <CopyField value="npm install -g @cori-do/cli" />
          <CopyField value={`cori login ${location.origin}${slug ? ` --org ${slug}` : ""}`} />
          <p className="text-[13px] text-fg-3">macOS and Linux (Windows through WSL). In a sandbox or CI: <code className="text-fg-2">npx @cori-do/cli …</code></p>
        </Card>
      </Section>

      <Sessions />
    </>
  );
}

function Agents() {
  const [host, setHost] = useState<Host>("claude-code");
  const mcp = authConfig().mcp || `${location.origin}/mcp`;
  const plugin = authConfig().agent_plugin;
  // the client agents sign in with when the identity provider registers none for them: Claude Code's command names it,
  // other agents ask for it
  const client = authConfig().mcp_client_id;
  const cursor = `cursor://anysphere.cursor-deeplink/mcp/install?name=cori&config=${encodeURIComponent(btoa(JSON.stringify({ url: mcp })))}`;
  const body: Record<Host, ReactNode> = {
    "claude-code": <>
      {plugin && <><p>With {brand.name}'s plugin: this server and the skill to build apps, in one install.</p><CopyField value={plugin} /><p className="pt-1">Or the server alone:</p></>}
      <CopyField value={claudeMcpAdd(mcp)} />
    </>,
    claude: <>
      <p>In claude.ai, Claude Desktop or Cowork: <b className="font-medium text-fg">Settings › Connectors › Add custom connector</b>, with this address:</p>
      <CopyField value={mcp} />
    </>,
    cursor: <>
      <ButtonLink href={cursor} variant="primary" icon={<ExternalLink />}>Add to Cursor</ButtonLink>
      <p className="pt-1">Or in <code className="text-fg-2">~/.cursor/mcp.json</code>:</p>
      <CopyField value={JSON.stringify({ mcpServers: { cori: { url: mcp } } })} />
    </>,
    vscode: <CopyField value={`code --add-mcp '${JSON.stringify({ name: "cori", type: "http", url: mcp })}'`} />,
    other: <>
      <p>Any MCP host that speaks streamable HTTP and OAuth finds how to sign you in at this address:</p>
      <CopyField value={mcp} />
    </>,
  };
  return (
    <Card className="p-5">
      <div className="no-scrollbar -mx-1 overflow-x-auto px-1">
        <Segmented label="Coding agent" value={host} onChange={setHost} options={[
          { value: "claude-code", label: "Claude Code" }, { value: "claude", label: "Claude app" }, { value: "cursor", label: "Cursor" }, { value: "vscode", label: "VS Code" }, { value: "other", label: "Other" },
        ]} />
      </div>
      <div className="mt-4 space-y-2.5 text-[14px] leading-relaxed text-fg-2">
        {body[host]}
        {client && host !== "claude-code" && <>
          <p className="pt-1">{host === "claude" ? <>Under <b className="font-medium text-fg">Advanced settings</b>, the OAuth Client ID:</> : "When it asks for an OAuth client ID, give it this one:"}</p>
          <CopyField value={client} />
        </>}
      </div>
    </Card>
  );
}

/** The agent sessions the person holds (admins: everyone's), each revocable. */
function Sessions() {
  const { me } = useSession();
  const q = useQuery<AgentSession[]>("/v1/agent-sessions", () => api<AgentSession[]>("/v1/agent-sessions"));
  const [showOld, setShowOld] = useState(false);
  if (q.error) return null;
  const now = Date.now();
  const active = (x: AgentSession) => !x.revoked_at && Date.parse(x.expires_at) > now;
  const rows = (q.data || []).filter((x) => showOld || active(x));
  const old = (q.data || []).filter((x) => !active(x)).length;
  const others = (q.data || []).some((x) => x.sub !== me?.sub);
  async function revoke(x: AgentSession) {
    if (!(await confirm({ title: "Revoke this agent session?", confirm: "Revoke", danger: true, body: <p>The agent loses its access to <b>{x.app}</b> at once. It asks you to sign in again the next time it needs it.</p> }))) return;
    try { await del(`/v1/agent-sessions/${encodeURIComponent(x.jti)}`); toast.success("Session revoked"); q.reload(); } catch (e) { toast.error(errMsg(e)); }
  }
  return (
    <Section title={others ? "Agent sessions" : "Your agent sessions"} description="Each app an agent works on gets a session of its own. Revoke one you don't recognize."
      actions={old > 0 && <button type="button" className="cursor-pointer text-[13px] text-fg-3 hover:text-fg" onClick={() => setShowOld(!showOld)}>{showOld ? "Hide ended ones" : `Show ${old} ended`}</button>}>
      {q.loading && !q.data ? <SkeletonRows rows={2} /> : rows.length === 0 ? (
        <Empty compact icon={<Bot />} title="No active agent session">An agent gets one when it starts working on an app.</Empty>
      ) : (
        <List>
          {rows.map((x) => {
            const on = active(x);
            return (
              <div key={x.jti} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 font-medium text-fg">
                    <a href={href("app", x.app)} className="hover:underline">{x.app}</a>
                    {!on && <Badge>{x.revoked_at ? "Revoked" : "Expired"}</Badge>}
                  </div>
                  <div className={cx("text-[13px] text-fg-3")}>
                    {others && <><PersonName id={x.sub} /> · </>}Started <Time at={x.issued_at}>{ago(x.issued_at)}</Time>{on && <> · expires {day(x.expires_at)}</>}
                  </div>
                </div>
                {on && <Button size="xs" variant="danger-soft" onClick={() => revoke(x)}>Revoke</Button>}
              </div>
            );
          })}
        </List>
      )}
    </Section>
  );
}
