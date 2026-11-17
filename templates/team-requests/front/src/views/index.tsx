import { ClipboardList, Inbox, Plus } from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { Approvals } from "./Approvals";
import { NewRequest } from "./NewRequest";
import { Requests } from "./Requests";

export type View = { id: string; label: string; icon: ReactNode; component: ComponentType; roles?: string[] };

/** One entry per screen, in the navigation's order; `roles` keeps a screen to the people holding one of them. Routing is
 *  on the hash (`#/requests`): the server falls back to index.html for unknown paths. */
export const VIEWS: View[] = [
  { id: "new", label: "New request", icon: <Plus className="size-4" aria-hidden />, component: NewRequest, roles: ["member", "manager"] },
  { id: "requests", label: "Requests", icon: <ClipboardList className="size-4" aria-hidden />, component: Requests, roles: ["member", "manager"] },
  { id: "approvals", label: "Approvals", icon: <Inbox className="size-4" aria-hidden />, component: Approvals, roles: ["manager"] },
];
