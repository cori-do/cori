import { useState, type ReactNode } from "react";
import { Search } from "lucide-react";
import { personName, personOf, usePeople, type Known, type Person } from "../lib/people";
import { Avatar, Button, cx, Input } from "./core";
import { Modal } from "./dialog";

/** An id as the person it names: their name, their address on hover (from the people, else from what the list said of
 *  them, `known`); the id when nobody is known by it. */
export function PersonName({ id, fallback, known }: { id?: string | null; fallback?: ReactNode; known?: Known }) {
  usePeople();
  if (!id) return <>{fallback ?? null}</>;
  const p: Known | undefined = personOf(id) ?? known;
  const label = personName(id, known);
  const title = [p?.name ? p.email : null, id !== label ? id : null].filter(Boolean).join(" · ");
  return <span title={title || undefined}>{label}</span>;
}

/** A person with their avatar. */
export function PersonChip({ id, name, email, guest, size = "sm", trailing }: { id: string; name?: string | null; email?: string | null; guest?: boolean; size?: "sm" | "md"; trailing?: ReactNode }) {
  usePeople();
  const p = personOf(id);
  const label = name || p?.name || email || p?.email || id;
  const sub = (name || p?.name) ? (email || p?.email) : null;
  return (
    <span className="inline-flex min-w-0 items-center gap-2.5">
      <Avatar name={label} size={size === "md" ? "md" : "sm"} />
      <span className="min-w-0">
        <span className="flex items-center gap-1.5 truncate text-fg">{label}{(guest ?? p?.guest) && <span className="rounded bg-amber-100 px-1 text-[11px] font-medium text-amber-800 dark:bg-amber-500/15 dark:text-amber-300">guest</span>}</span>
        {size === "md" && sub && <span className="block truncate text-[13px] text-fg-3">{sub}</span>}
      </span>
      {trailing}
    </span>
  );
}

const match = (p: Person, q: string) => !q || `${p.name || ""} ${p.email || ""} ${p.id}`.toLowerCase().includes(q.toLowerCase());

function PeopleList({ rows, picked, toggle, single, empty }: { rows: Person[]; picked: Set<string>; toggle: (id: string) => void; single?: boolean; empty: ReactNode }) {
  return (
    <div className="max-h-72 overflow-y-auto rounded-xl ring-1 ring-line">
      {rows.map((p) => (
        <label key={p.id} className={cx("flex cursor-pointer items-center gap-3 border-b border-line px-3 py-2 last:border-0", picked.has(p.id) ? "bg-brand-50/60 dark:bg-brand-500/10" : "hover:bg-surface-2")}>
          <input type={single ? "radio" : "checkbox"} name="person" checked={picked.has(p.id)} onChange={() => toggle(p.id)} className="size-4 shrink-0" />
          <PersonChip id={p.id} name={p.name} email={p.email} guest={p.guest} size="md" />
        </label>
      ))}
      {rows.length === 0 && <div className="px-3 py-6 text-center text-fg-3">{empty}</div>}
    </div>
  );
}

function SearchBox({ q, setQ }: { q: string; setQ: (s: string) => void }) {
  return (
    <div className="relative">
      <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-fg-4" aria-hidden />
      <Input autoFocus className="pl-9" placeholder="Search people" aria-label="Search people" value={q} onChange={(e) => setQ(e.target.value)} />
    </div>
  );
}

/** Picks people among the organization's members (`chosen` ticked); ids nobody knows stay listed so they can be removed. */
export function PeoplePicker({ title, description, chosen, exclude = [], confirm = "Save", close }: {
  title: ReactNode; description?: ReactNode; chosen: string[]; exclude?: string[]; confirm?: string; close: (v: string[] | null) => void;
}) {
  const people = usePeople().filter((p) => !p.guest);
  const [picked, setPicked] = useState<Set<string>>(() => new Set(chosen));
  const [q, setQ] = useState("");
  const known = new Set(people.map((p) => p.id));
  const rows = [...people, ...chosen.filter((id) => !known.has(id)).map((id) => ({ id }) as Person)].filter((p) => !exclude.includes(p.id) && match(p, q));
  const toggle = (id: string) => setPicked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  return (
    <Modal title={title} description={description} onClose={() => close(null)} onSubmit={() => close([...picked])}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary">{confirm}</Button></>}>
      <SearchBox q={q} setQ={setQ} />
      <PeopleList rows={rows} picked={picked} toggle={toggle} empty={q ? "Nobody matches." : "Nobody else has signed in yet."} />
    </Modal>
  );
}

/** Picks one member (no guest). */
export function PersonPicker({ title, description, exclude = [], confirm = "Choose", close }: { title: ReactNode; description?: ReactNode; exclude?: string[]; confirm?: string; close: (v: string | null) => void }) {
  const people = usePeople().filter((p) => !p.guest && !exclude.includes(p.id));
  const [picked, setPicked] = useState<string | null>(null);
  const [q, setQ] = useState("");
  return (
    <Modal title={title} description={description} onClose={() => close(null)} onSubmit={() => picked && close(picked)}
      footer={<><Button variant="ghost" onClick={() => close(null)}>Cancel</Button><Button type="submit" variant="primary" disabled={!picked}>{confirm}</Button></>}>
      <SearchBox q={q} setQ={setQ} />
      <PeopleList single rows={people.filter((p) => match(p, q))} picked={new Set(picked ? [picked] : [])} toggle={setPicked} empty={q ? "Nobody matches." : "Nobody else is known here yet."} />
    </Modal>
  );
}
