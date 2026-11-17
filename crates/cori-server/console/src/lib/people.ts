import { api } from "./api";
import { peek, useQuery } from "./query";
import { useSession } from "./session";
import type { PeopleList } from "./types";

// The people of the organization as the server knows them (`GET /v1/people`): the ids the platform keeps (owners,
// builders, runs' users, deciders, the audit log) read as names. Those who reach their apps only (guests, people who build
// nothing and hold no power) and older servers get none: ids stay ids, or the names a list gives with them (`known`).

/** A person: their id (the token's subject), address and name, when the server first and last saw them, whether a guest. */
export type Person = { id: string; email?: string | null; name?: string | null; first_seen?: string; last_seen?: string; guest?: boolean };

const KEY = "/v1/people";
const fetchPeople = () => api<PeopleList>(KEY).catch(() => ({ people: [] }) as unknown as PeopleList);
const index = new WeakMap<object, Map<string, Person>>();

function byId(list?: PeopleList): Map<string, Person> {
  if (!list) return new Map();
  let m = index.get(list);
  if (!m) { m = new Map((list.people || []).map((p) => [p.id, p as Person])); index.set(list, m); }
  return m;
}

/** The people list (with what the caller may change), re-rendering when it loads again; asked once the console knows the
 *  person may read it. */
export function usePeopleList() {
  const s = useSession();
  return useQuery<PeopleList>(s.me && !s.appsOnly ? KEY : null, fetchPeople);
}
export const usePeople = (): Person[] => (usePeopleList().data?.people as Person[] | undefined) ?? [];

export const personOf = (id?: string | null) => (id ? byId(peek<PeopleList>(KEY)).get(id) : undefined);

/** What a list says of a person beside their id (an approval's `requester`). */
export type Known = { name?: string | null; email?: string | null };

/** How an id reads: the person's name, else their address (from the people, else from `known`); a schedule or a service in
 *  words; else the id itself. */
export function personName(id?: string | null, known?: Known): string {
  if (!id) return "";
  const p = personOf(id);
  if (p?.name || p?.email) return (p.name || p.email)!;
  if (known?.name || known?.email) return (known.name || known.email)!;
  if (id === "schedule") return "Schedule";
  if (id.startsWith("service:")) return `${id.slice("service:".length)} (service)`;
  return id;
}
