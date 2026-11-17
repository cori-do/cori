import { useState } from "react";
import { Plus } from "lucide-react";
import { api, errMsg, post } from "../../lib/api";
import { useQuery } from "../../lib/query";
import type { Secret } from "../../lib/types";
import { Button, ErrorNote, Section, Segmented, SkeletonRows } from "../../ui/core";
import { openDialog } from "../../ui/dialog";
import { SecretForm, SecretsList } from "../../ui/secrets";
import { toast } from "../../ui/toast";

// Every credential the organization keeps: the ones every app may use, and each app's own. Values are sealed and never
// shown; a run looks its app's up first, then the organization's.

export function Credentials() {
  const q = useQuery<Secret[]>("/v1/secrets", () => api<Secret[]>("/v1/secrets"));
  const [scope, setScope] = useState<"shared" | "apps">("shared");
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={3} />;
  const shared = q.data.filter((s) => !s.app);
  const apps = q.data.filter((s) => s.app);
  async function add() {
    const v = await openDialog<{ name: string; value: string }>((close) => <SecretForm scope="every app of the organization" close={close} />);
    if (!v) return;
    try { await post("/v1/secrets", { name: v.name, value: v.value }); toast.success(`${v.name} saved`); } catch (e) { toast.error(errMsg(e)); }
    q.reload();
  }
  return (
    <Section actions={scope === "shared" && <Button size="xs" icon={<Plus />} onClick={add}>Add</Button>}
      title={<Segmented label="Credentials of" value={scope} onChange={setScope} options={[{ value: "shared", label: "Every app", count: shared.length }, { value: "apps", label: "One app", count: apps.length }]} />}>
      {scope === "shared"
        ? <SecretsList secrets={shared} empty={<>Add one here, or with <code>cori secrets set &lt;name&gt;</code>.</>} />
        : <SecretsList secrets={apps} showApp empty={<>An app's own credentials are added in its settings, or with <code>cori secrets set &lt;name&gt; --app &lt;app&gt;</code>.</>} />}
    </Section>
  );
}
