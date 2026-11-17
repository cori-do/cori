import { Home as HomeIcon } from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { Home } from "./Home";

export type View = { id: string; label: string; icon: ReactNode; component: ComponentType; roles?: string[] };

/** One entry per screen, in the navigation's order; `roles` keeps a screen to the people holding one of them. Routing is
 *  on the hash (`#/home`): the server falls back to index.html for unknown paths. */
export const VIEWS: View[] = [
  { id: "home", label: "Home", icon: <HomeIcon className="size-4" aria-hidden />, component: Home },
];
