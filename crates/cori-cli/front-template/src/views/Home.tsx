import { useUser } from "@cori-do/client/react";
import { LayoutGrid } from "lucide-react";
import { coriConfig } from "../lib/api";
import { APP_NAME } from "../lib/app";
import { cori } from "../lib/cori";
import { Badge, Card, CardBody, CardHeader, EmptyState, Spinner } from "../components/ui";

/** The starting screen. Replace it with the app's first real screen, and add one file per screen in views/. */
export function Home() {
  const user = useUser(cori);
  const cfg = coriConfig();
  const workflows = cfg.dev?.workflows.filter((w) => w.endpoint) ?? [];
  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{APP_NAME}</h1>
        <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">Signed in as {user.loading ? "…" : user.sub ?? "nobody"}.</p>
      </div>
      <Card>
        <CardHeader title="Workflows" description="What this app can run, as the server reports it under dev auth." />
        {user.loading ? (
          <Spinner />
        ) : workflows.length === 0 ? (
          <EmptyState icon={<LayoutGrid className="size-5" />} title="No screens yet" description="Add a workflow with an endpoint, run `npm run gen`, then build a screen for it in src/views/." />
        ) : (
          <CardBody>
            <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
              {workflows.map((w) => (
                <li key={w.id} className="flex flex-wrap items-center justify-between gap-2 py-3">
                  <div>
                    <p className="font-mono text-sm">{w.id}</p>
                    {w.description && <p className="text-xs text-zinc-500 dark:text-zinc-400">{w.description}</p>}
                  </div>
                  <div className="flex gap-1.5">
                    <Badge tone="brand">{w.endpoint!.mode}</Badge>
                    {w.endpoint!.roles.map((r) => <Badge key={r}>{r}</Badge>)}
                  </div>
                </li>
              ))}
            </ul>
          </CardBody>
        )}
      </Card>
    </div>
  );
}
