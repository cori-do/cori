import { useEffect, useState, type ReactNode } from "react";
import { Download, ExternalLink } from "lucide-react";
import { api, authMode, del, download, errMsg, patch, signOut, switchOrganization } from "../../lib/api";
import { brand } from "../../lib/brand";
import { invalidate, useQuery } from "../../lib/query";
import type { AccessRequest, OrganizationInfo } from "../../lib/types";
import { leaveOrganization, outcome } from "../../ui/access";
import { Button, Callout, Card, ErrorNote, Input, Section, SkeletonRows } from "../../ui/core";
import { confirm } from "../../ui/dialog";
import { toast } from "../../ui/toast";

// The organization's name and address (as its identity provider allows: changing the slug moves every app's address), its
// export, leaving it, and deleting it.

export function General() {
  const q = useQuery<OrganizationInfo>("/v1/organization", () => api<OrganizationInfo>("/v1/organization"));
  if (q.error && !q.data) return <ErrorNote retry={q.reload}>{q.error}</ErrorNote>;
  if (!q.data) return <SkeletonRows rows={2} />;
  const org = q.data;
  const can = org.can || {};
  const label = org.name || org.slug || "your organization";
  const after = () => invalidate("/v1/organization", "/v1/me");
  const say = ([msg, bad]: [string, boolean]) => (bad ? toast.error : toast.success)(msg);

  async function rename(name: string) {
    try { say(outcome("The organization's name", await patch<AccessRequest>("/v1/organization", { name }))); } catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function reslug(slug: string) {
    if (!(await confirm({ title: `Change the address to ${slug}?`, confirm: "Change it",
      body: <p>Every app's address moves to <code>{"<app>--"}{slug}</code>, and its sign-in follows. Links to the old addresses stop working.</p> }))) return;
    try {
      const r = await patch<AccessRequest>("/v1/organization", { slug });
      say(outcome("The organization's address", r));
      if (r.status === "done" && authMode() === "builtin") { await switchOrganization(slug); return; }
    } catch (e) { toast.error(errMsg(e)); }
    after();
  }
  async function remove() {
    const want = org.slug || org.id || "";
    if (!(await confirm({ title: `Delete ${label}?`, danger: true, confirm: "Delete the organization", typeToConfirm: want,
      body: <p>Its apps, runs, data, releases and credentials are deleted for good, and its people leave it (their accounts stay). Download its export first to keep anything.</p> }))) return;
    try { await del(`/v1/organization?confirm=${encodeURIComponent(want)}`); toast.success(`${label} is being deleted`); await signOut(); }
    catch (e) { toast.error(errMsg(e)); }
  }
  async function exportIt() {
    try { toast("Preparing the export…"); await download("/v1/export", `${brand.name.toLowerCase()}-export.tar.zst`); }
    catch (e) { toast.error(errMsg(e)); }
  }

  return (
    <div className="max-w-2xl">
      {/* a server of one organization has no name to show or change: only its export and the rest */}
      {(org.name || org.slug || can.rename) && <>
        {!can.rename && (
          <Callout className="mb-4" action={org.admin_url && <a href={org.admin_url} target="_blank" rel="noopener noreferrer" className="link inline-flex items-center gap-1 text-[13px] font-medium">Open it<ExternalLink className="size-3.5" /></a>}>
            Its name{org.slug ? " and address are" : " is"} changed in your identity provider.
          </Callout>
        )}
        <Card className="divide-y divide-line">
          <Setting label="Name" hint="How the console and emails name it." value={org.name || ""} editable={!!can.rename} onSave={rename} />
          {org.id && org.slug && <Setting label="Address" mono value={org.slug} editable={!!can.rename} onSave={reslug}
            hint={<>Part of every app's address: <code>{"<app>--"}{org.slug}</code>. Changing it moves them all.</>} />}
        </Card>
      </>}

      {can.export && (
        <Section title="Export" className={org.name || org.slug || can.rename ? "mt-8" : ""}>
          <Card className="flex flex-wrap items-center gap-4 p-5">
            <p className="min-w-0 flex-1 text-[14px] text-fg-2">Its apps, releases, runs, approvals, data and audit log, as one archive.</p>
            <Button icon={<Download />} onClick={exportIt}>Download</Button>
          </Card>
        </Section>
      )}

      {(can.leave || can.delete) && (
        <Section title="Leave or delete" className="mt-8">
          <Card className="divide-y divide-line">
            {can.leave && (
              <div className="flex flex-wrap items-center gap-4 px-5 py-4">
                <p className="min-w-0 flex-1 text-[14px] text-fg-2">Leave {label}: your roles in it and in its apps go.</p>
                <Button variant="danger-soft" onClick={() => leaveOrganization(label)}>Leave</Button>
              </div>
            )}
            {can.delete && (
              <div className="flex flex-wrap items-center gap-4 px-5 py-4">
                <p className="min-w-0 flex-1 text-[14px] text-fg-2">Delete {label} and everything in it, for good.</p>
                <Button variant="danger" onClick={remove}>Delete…</Button>
              </div>
            )}
          </Card>
        </Section>
      )}
    </div>
  );
}

/** A setting edited in place: its value, and Save once it changed. */
function Setting({ label, value, editable, onSave, hint, mono }: { label: string; value: string; editable: boolean; onSave: (v: string) => void; hint?: ReactNode; mono?: boolean }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  const changed = v.trim() !== value && !!v.trim();
  return (
    <form className="flex flex-wrap items-start gap-x-6 gap-y-2 px-5 py-4" onSubmit={(e) => { e.preventDefault(); if (changed) onSave(v.trim()); }}>
      <div className="w-28 shrink-0 pt-2 text-[14px] font-medium text-fg">{label}</div>
      <div className="min-w-0 flex-1 space-y-1.5">
        {editable ? <Input className={mono ? "font-mono" : ""} value={v} onChange={(e) => setV(e.target.value)} aria-label={label} /> : <div className={`pt-2 ${mono ? "font-mono" : ""}`}>{value || "—"}</div>}
        {hint && <p className="text-[13px] text-fg-3">{hint}</p>}
      </div>
      {editable && <Button type="submit" variant="primary" disabled={!changed} className="mt-0.5">Save</Button>}
    </form>
  );
}
