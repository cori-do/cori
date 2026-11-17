import { useState } from "react";
import { MoreHorizontal, Plus, RefreshCw, Sparkles, Trash2 } from "lucide-react";
import { api, del, errMsg, put } from "../../lib/api";
import { ago, eur, plural, starterCalls } from "../../lib/format";
import { invalidate, useQuery } from "../../lib/query";
import { href } from "../../lib/router";
import type { LlmModel, LlmMonth, LlmOverview, LlmProvider, LlmTier } from "../../lib/types";
import { Badge, Button, Callout, Card, cx, Empty, ErrorNote, Field, IconButton, Input, List, Section, Select, SkeletonRows } from "../../ui/core";
import { confirm, Modal, openDialog } from "../../ui/dialog";
import { Menu } from "../../ui/menu";
import { toast } from "../../ui/toast";

// The models AI steps run on: the organization's providers (keys write-only), the default model per tier, and what each
// app may spend a month. Admins change them; operators and billing read them.

const KIND_LABEL: Record<string, string> = { openai: "OpenAI-compatible", anthropic: "Anthropic" };
const DEFAULT_URL: Record<string, string> = { openai: "https://api.openai.com/v1", anthropic: "https://api.anthropic.com/v1" };
const TIER_HINT: Record<string, string> = { small: "Quick, cheap steps: classify, extract", medium: "Most steps", large: "Hard reasoning and long documents" };

export function Models() {
  const q = useQuery<LlmOverview>("/v1/llm", () => api<LlmOverview>("/v1/llm"));
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={3} />;
  const data = q.data;
  const edit = data.can_edit;
  return (
    <>
      {!edit && <Callout className="mb-4">Admins change these; you can read them.</Callout>}
      {data.starter && (
        <Card className="flex gap-4 p-5">
          <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-brand-50 text-brand-700 dark:bg-brand-500/15 dark:text-brand-100"><Sparkles className="size-5" /></span>
          <div className="min-w-0 flex-1 text-[14px] leading-relaxed">
            <div className="flex flex-wrap items-center gap-2 font-semibold text-fg">Starter model <code className="font-normal text-fg-3">{data.starter.model}</code>
              {data.starter.calls_per_day > 0 && <Badge tone={data.starter.calls_today >= data.starter.calls_per_day ? "warning" : "neutral"}>{data.starter.calls_today} of {data.starter.calls_per_day} calls today</Badge>}</div>
            <p className="mt-1 text-fg-2">
              Free to use: {starterCalls(data.starter)}. {(data.starter.tiers ?? []).length ? <>It answers the {(data.starter.tiers ?? []).join(", ")} tier{(data.starter.tiers ?? []).length > 1 ? "s" : ""} until you choose a model of your own.</> : "Every tier has a model of yours: it answers none."}
            </p>
            {data.starter.note && <p className="mt-1 text-[13px] text-fg-3">{data.starter.note}</p>}
          </div>
        </Card>
      )}
      <Providers data={data} edit={edit} />
      <Tiers data={data} edit={edit} />
      <Budgets apps={data.apps} />
    </>
  );
}

function Providers({ data, edit }: { data: LlmOverview; edit: boolean }) {
  const open = async (p?: LlmProvider) => { if (await openDialog<true>((close) => <ProviderDialog p={p} kinds={data.kinds} taken={data.providers.map((x) => x.name)} close={close} />)) invalidate("/v1/llm"); };
  const remove = async (p: LlmProvider) => {
    if (!(await confirm({ title: `Delete ${p.name}?`, confirm: "Delete", danger: true, body: <p>Its key is deleted. AI steps of apps that use it stop reaching a model.</p> }))) return;
    try { await del(`/v1/llm/providers/${p.name}`); toast.success(`${p.name} deleted`); } catch (e) { toast.error(errMsg(e)); }
    invalidate("/v1/llm");
  };
  return (
    <Section title="Providers" description="Where AI steps get their models, with your organization's own keys." actions={edit && <Button size="xs" icon={<Plus />} onClick={() => open()}>Add provider</Button>}>
      {data.providers.length ? (
        <List>
          {data.providers.map((p) => (
            <div key={p.name} className="flex flex-wrap items-start gap-x-4 gap-y-2 px-4 py-3.5">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium text-fg">{p.name}</span>
                  <span className="text-[13px] text-fg-3">{KIND_LABEL[p.kind] || p.kind}</span>
                  {!p.enabled && <Badge>Disabled</Badge>}
                  {p.has_key ? <Badge tone="success" dot>Key set</Badge> : <Badge tone="warning" dot title="only a keyless endpoint answers">No key</Badge>}
                  {p.managed && <Badge tone="brand" title="set by the deployment's providers file: change it there">Managed by the deployment</Badge>}
                </div>
                <div className="mt-1 truncate text-[13px] text-fg-3">{p.base_url || DEFAULT_URL[p.kind]}</div>
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {p.models.map((m) => (
                    <span key={m.id} className="inline-flex items-center gap-1.5 rounded-md bg-surface-2 px-2 py-0.5 text-[12.5px] ring-1 ring-line ring-inset" title={`€ per million tokens: ${m.input_eur_per_mtok} in / ${m.output_eur_per_mtok} out`}>
                      <code className="text-fg">{m.id}</code>
                      <span className={cx("tabular-nums", !m.input_eur_per_mtok && !m.output_eur_per_mtok ? "text-amber-700 dark:text-amber-300" : "text-fg-3")}>{!m.input_eur_per_mtok && !m.output_eur_per_mtok ? "no price" : `${m.input_eur_per_mtok} / ${m.output_eur_per_mtok} €`}</span>
                    </span>
                  ))}
                  {!p.models.length && <span className="text-[13px] text-fg-4">No model yet</span>}
                </div>
              </div>
              <span className="text-[13px] text-fg-4">{p.managed ? "" : `Updated ${ago(p.updated_at)}`}</span>
              {edit && !p.managed && (
                <Menu items={[{ label: "Edit…", onSelect: () => open(p) }, "sep", { label: "Delete", icon: <Trash2 />, danger: true, onSelect: () => remove(p) }]}>
                  {(t) => <IconButton {...t} label={`Actions for ${p.name}`} size="xs"><MoreHorizontal /></IconButton>}
                </Menu>
              )}
            </div>
          ))}
        </List>
      ) : (
        <Empty compact icon={<Sparkles />} title="No provider yet" action={edit && <Button variant="primary" icon={<Plus />} onClick={() => open()}>Add provider</Button>}>
          OpenAI, Anthropic, or any OpenAI-compatible endpoint (vLLM, Mistral…).{data.starter ? " Until then, the starter model answers." : ""}
        </Empty>
      )}
    </Section>
  );
}

type Row = { id: string; input: string; output: string };
const toRows = (ms: LlmModel[]): Row[] => ms.map((m) => ({ id: m.id, input: String(m.input_eur_per_mtok ?? ""), output: String(m.output_eur_per_mtok ?? "") }));

function ProviderDialog({ p, kinds, taken, close }: { p?: LlmProvider; kinds: string[]; taken: string[]; close: (v: true | null) => void }) {
  const [name, setName] = useState(p?.name || "");
  const [kind, setKind] = useState(p?.kind || "openai");
  const [baseUrl, setBaseUrl] = useState(p?.base_url || "");
  const [key, setKey] = useState("");
  const [enabled, setEnabled] = useState(p?.enabled ?? true);
  const [rows, setRows] = useState<Row[]>(p ? toRows(p.models) : [{ id: "", input: "", output: "" }]);
  const [offered, setOffered] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [fetching, setFetching] = useState(false);
  const [err, setErr] = useState("");
  const set = (i: number, k: keyof Row, v: string) => setRows(rows.map((r, j) => (j === i ? { ...r, [k]: v } : r)));
  const available = offered.filter((m) => !rows.some((r) => r.id.trim() === m));
  const addModels = (ids: string[]) => setRows([...rows.filter((r) => r.id.trim()), ...ids.map((id) => ({ id, input: "", output: "" }))]);
  const body = () => ({
    kind, base_url: baseUrl.trim() || null, enabled, ...(key.trim() ? { api_key: key.trim() } : {}),
    models: rows.filter((r) => r.id.trim()).map((r) => ({ id: r.id.trim(), input_eur_per_mtok: Number(r.input || 0), output_eur_per_mtok: Number(r.output || 0) })),
  });
  const invalid = () => {
    if (!/^[A-Za-z0-9_-]+$/.test(name) || name === "mock") return "Name: letters, digits, - and _ (not “mock”)";
    if (!p && taken.includes(name)) return `${name} exists already`;
    if (rows.some((r) => r.id.trim() && [r.input, r.output].some((x) => x !== "" && !(Number(x) >= 0)))) return "Prices are amounts of at least 0";
    return "";
  };
  const save = async () => {
    const bad = invalid();
    if (bad) return setErr(bad);
    setBusy(true);
    try { await put(`/v1/llm/providers/${name}`, body()); toast.success(`${name} saved`); close(true); } catch (e) { setErr(errMsg(e)); } finally { setBusy(false); }
  };
  // saves first (the key typed here is what the provider is asked with), then lists the provider's models
  const fetchModels = async () => {
    const bad = invalid();
    if (bad) return setErr(bad);
    setFetching(true); setErr("");
    try {
      await put(`/v1/llm/providers/${name}`, body());
      setKey("");
      const r = await api<{ models: string[] }>(`/v1/llm/providers/${name}/models`);
      setOffered(r.models);
      toast.success(r.models.length ? `The key works: ${name} offers ${r.models.length} models` : `The key works; ${name} listed no model`);
    } catch (e) { setErr(errMsg(e)); } finally { setFetching(false); }
  };
  return (
    <Modal size="lg" title={p ? `Edit ${p.name}` : "Add a provider"} onClose={() => close(null)} onSubmit={save}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary" loading={busy}>Save</Button></>}>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name"><Input value={name} disabled={!!p} autoFocus={!p} placeholder="openai" onChange={(e) => setName(e.target.value)} /></Field>
        <Field label="API">
          <Select value={kind} onChange={(e) => setKind(e.target.value)}>{kinds.map((k) => <option key={k} value={k}>{KIND_LABEL[k] || k}</option>)}</Select>
        </Field>
      </div>
      <Field label="Address" optional hint={`Leave empty for ${DEFAULT_URL[kind] || "the provider's own"}.`}><Input value={baseUrl} placeholder={DEFAULT_URL[kind]} onChange={(e) => setBaseUrl(e.target.value)} /></Field>
      <Field label={<>API key{p?.has_key && <span className="ml-1 font-normal text-fg-3">(stored: leave empty to keep it)</span>}</>} hint="Encrypted and never shown again; a run gets it for one call.">
        <Input className="font-mono" type="password" autoComplete="off" value={key} placeholder={p?.has_key ? "••••••••" : "sk-…"} onChange={(e) => setKey(e.target.value)} />
      </Field>
      <div>
        <div className="mb-2 flex items-center justify-between gap-2">
          <span className="text-[13px] font-medium text-fg">Models <span className="font-normal text-fg-3">· € per million tokens</span></span>
          <Button size="xs" variant="ghost" icon={<RefreshCw />} loading={fetching} onClick={fetchModels} disabled={!p?.has_key && !key.trim()}>Test the key and list models</Button>
        </div>
        <datalist id="llm-offered">{offered.map((m) => <option key={m} value={m} />)}</datalist>
        <div className="space-y-1.5">
          <div className="grid grid-cols-[1fr_5.5rem_5.5rem_2rem] gap-1.5 text-xs text-fg-3"><span>Model</span><span>Input</span><span>Output</span><span /></div>
          {rows.map((r, i) => (
            <div key={i} className="grid grid-cols-[1fr_5.5rem_5.5rem_2rem] gap-1.5">
              <Input h="sm" list="llm-offered" className="font-mono" value={r.id} placeholder="model id" onChange={(e) => set(i, "id", e.target.value)} />
              <Input h="sm" className="tabular-nums" inputMode="decimal" value={r.input} placeholder="0" onChange={(e) => set(i, "input", e.target.value)} aria-label={`${r.id || "model"} input price`} />
              <Input h="sm" className="tabular-nums" inputMode="decimal" value={r.output} placeholder="0" onChange={(e) => set(i, "output", e.target.value)} aria-label={`${r.id || "model"} output price`} />
              <IconButton label="Remove the model" onClick={() => setRows(rows.filter((_, j) => j !== i))}><Trash2 /></IconButton>
            </div>
          ))}
          <Button size="xs" variant="ghost" icon={<Plus />} onClick={() => setRows([...rows, { id: "", input: "", output: "" }])}>Add a model</Button>
        </div>
        {available.length > 0 && (
          <div className="mt-2 rounded-xl bg-surface-2 p-3 ring-1 ring-line ring-inset">
            <div className="mb-2 flex items-center justify-between gap-2 text-[13px] text-fg-3"><span>{name} offers {available.length} more: pick the ones apps may use, then set their prices</span><Button size="xs" variant="ghost" onClick={() => addModels(available)}>Add all</Button></div>
            <div className="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
              {available.map((m) => (
                <button key={m} type="button" onClick={() => addModels([m])} className="inline-flex cursor-pointer items-center gap-1 rounded-md bg-surface px-1.5 py-0.5 font-mono text-xs text-fg-2 ring-1 ring-line hover:ring-brand-500">
                  <Plus className="size-3" />{m}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
      <label className="flex cursor-pointer items-center gap-2.5">
        <input type="checkbox" className="size-4" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
        <span>Enabled <span className="text-fg-3">· a disabled provider answers no call</span></span>
      </label>
      {err && <p className="text-[13px] text-rose-600 dark:text-rose-400">{err}</p>}
    </Modal>
  );
}

function Tiers({ data, edit }: { data: LlmOverview; edit: boolean }) {
  const saved = (t: string) => (data.tiers[t] ? `${data.tiers[t].provider}/${data.tiers[t].model}` : "");
  const [draft, setDraft] = useState<Record<string, string>>(() => Object.fromEntries(data.tier_names.map((t) => [t, saved(t)])));
  const [busy, setBusy] = useState(false);
  const enabled = data.providers.filter((p) => p.enabled);
  // a model's name alone when there is one provider; with its provider's otherwise
  const choices = enabled.flatMap((p) => p.models.map((m) => ({ value: `${p.name}/${m.id}`, label: enabled.length > 1 ? `${p.name} · ${m.id}` : m.id })));
  const dirty = data.tier_names.some((t) => draft[t] !== saved(t));
  const save = async () => {
    setBusy(true);
    const body: Record<string, LlmTier | null> = {};
    for (const t of data.tier_names) { const v = draft[t]; const i = v.indexOf("/"); body[t] = v ? { provider: v.slice(0, i), model: v.slice(i + 1) } : null; }
    try { await put("/v1/llm/tiers", body); toast.success("Default models saved"); } catch (e) { toast.error(errMsg(e)); } finally { setBusy(false); }
    invalidate("/v1/llm");
  };
  return (
    <Section title="Default models" description="Each AI step asks for a tier. Apps that pick no model of their own get these." actions={edit && dirty && <Button size="xs" variant="primary" loading={busy} onClick={save}>Save</Button>}>
      <div className="grid gap-3 sm:grid-cols-3">
        {data.tier_names.map((t) => (
          <Card key={t} className="p-4">
            <div className="flex items-center gap-2 font-medium capitalize">{t}{data.tiers[t]?.managed && <Badge tone="brand">Deployment</Badge>}</div>
            <div className="mb-2.5 text-[13px] text-fg-3">{TIER_HINT[t] || ""}</div>
            <Select h="sm" value={draft[t]} disabled={!edit || !!data.tiers[t]?.managed} onChange={(e) => setDraft({ ...draft, [t]: e.target.value })} aria-label={`${t} model`}>
              <option value="">{data.starter ? `Starter model · ${data.starter.model}` : "None"}</option>
              {draft[t] && !choices.some((c) => c.value === draft[t]) && <option value={draft[t]}>{draft[t]} (unavailable)</option>}
              {choices.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
            </Select>
          </Card>
        ))}
      </div>
      {!choices.length && edit && <p className="mt-2 text-[13px] text-fg-3">Add a provider with a model to choose one.</p>}
    </Section>
  );
}

function Budgets({ apps }: { apps: LlmMonth[] }) {
  const month = apps[0]?.month;
  const blocked = apps.filter((a) => a.blocked);
  return (
    <Section title={`Spend this month${month ? ` · ${month}` : ""}`} description="Each app's AI spend against its monthly budget. Set a budget in the app's settings.">
      {blocked.length > 0 && <Callout tone="danger" className="mb-3">{blocked.map((a) => a.app).join(", ")} spent {blocked.length === 1 ? "its" : "their"} budget: AI steps fail until next month or a higher budget.</Callout>}
      {apps.length ? (
        <List>
          {apps.map((a) => {
            const pct = a.budget_eur ? Math.min(100, (a.spent_eur / a.budget_eur) * 100) : a.budget_eur === 0 ? 100 : 0;
            return (
              <a key={a.app} href={href("app", a.app || "", "settings")} className="flex flex-wrap items-center gap-x-6 gap-y-2 px-4 py-3 hover:bg-surface-2/60">
                <div className="w-44 min-w-0">
                  <div className="truncate font-medium text-fg">{a.app}</div>
                  <div className="text-[13px] text-fg-3">{plural(a.calls, "call")} · {(a.tokens_in + a.tokens_out).toLocaleString()} tokens</div>
                </div>
                <div className="min-w-[160px] flex-1">
                  {a.budget_eur == null ? <span className="text-[13px] text-fg-4">No budget</span> : (
                    <div className="h-1.5 overflow-hidden rounded-full bg-surface-3" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label={`${a.app} budget used`}>
                      <div className={cx("h-full rounded-full", a.blocked ? "bg-rose-500" : pct >= 80 ? "bg-amber-500" : "bg-emerald-500")} style={{ width: `${pct}%` }} />
                    </div>
                  )}
                </div>
                <div className="w-40 text-right text-[14px] tabular-nums"><span className="font-medium">{eur(a.spent_eur)}</span>{a.budget_eur != null && <span className="text-fg-3"> of {eur(a.budget_eur)}</span>}</div>
              </a>
            );
          })}
        </List>
      ) : <Empty compact title="No app yet" />}
      <p className="mt-2 text-[13px] text-fg-4">Calls this calendar month (UTC), at the prices set on the providers.</p>
    </Section>
  );
}
