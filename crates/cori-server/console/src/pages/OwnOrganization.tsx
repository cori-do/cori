import { useState } from "react";
import { Building2, Plus, X } from "lucide-react";
import { api } from "../lib/api";
import { useQuery } from "../lib/query";
import { useSession } from "../lib/session";
import type { MyOrganizations } from "../lib/types";
import { ButtonLink, Card, IconButton } from "../ui/core";

// Someone who may not create apps in this organization (a member without the builder role, a guest) builds them in an
// organization of their own, when the identity provider lets them start one (`GET /v1/me/organizations`'s `start`: its
// page, or why not). Shown on Home until they close it; the organizations menu keeps offering it.

const CLOSED = "cori.own-organization.closed";

export function OwnOrganization() {
  const { me } = useSession();
  const org = me?.organization;
  const [, closing] = useState(0);
  const key = org ? `${CLOSED}.${org.id}` : "";
  const closed = !!key && (() => { try { return localStorage.getItem(key) === "1"; } catch { return false; } })();
  const offered = !!me && !!org && !me.can?.create_apps && !closed;
  const q = useQuery<MyOrganizations>(offered ? "/v1/me/organizations" : null, () => api<MyOrganizations>("/v1/me/organizations"));
  const start = q.data?.start;
  if (!offered || !(start?.url || start?.reason)) return null;
  const name = org.name || org.slug || "this organization";
  const close = () => { try { localStorage.setItem(key, "1"); } catch { /* private mode */ } closing((n) => n + 1); };
  return (
    <Card as="section" className="mb-8 flex items-start gap-4 p-5">
      <span className="hidden size-10 shrink-0 items-center justify-center rounded-xl bg-brand-50 text-brand-700 sm:flex dark:bg-brand-500/15 dark:text-brand-100" aria-hidden><Building2 className="size-5" /></span>
      <div className="min-w-0 flex-1 text-[14px] leading-relaxed text-fg-2">
        <h2 className="text-[15px] font-semibold text-fg">Build apps of your own</h2>
        <p className="mt-0.5">{me.guest ? <>As a guest of {name}, you use the apps it shares with you.</> : <>Creating apps in {name} takes the builder role, which one of its admins gives.</>}</p>
        {start.url ? (
          <>
            <p>{me.guest
              ? <>To build yours, start an organization of your own: you own it, create and deploy apps there, and keep what {name} shares with you.</>
              : <>Or start an organization of your own: you own it, create and deploy apps there, and stay in {name}.</>}</p>
            <ButtonLink variant="primary" size="md" className="mt-3" icon={<Plus />} href={start.url}>Start an organization</ButtonLink>
          </>
        ) : <p className="mt-1 text-[13px] text-fg-3">{start.reason}</p>}
      </div>
      <IconButton label="Hide" size="xs" onClick={close}><X /></IconButton>
    </Card>
  );
}
