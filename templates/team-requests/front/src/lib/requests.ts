import type { Tone } from "../components/ui";

/** What `submit_request` asks a manager (the `approve_request` step's input). */
export type Ask = { request_id: string; title: string; details: string; amount_eur: number; category: string; summary: string; requester: string; reason: string };

export const STATUS_TONE: Record<string, Tone> = { pending: "amber", approved: "green", declined: "red" };

export const euros = (n: number) => new Intl.NumberFormat(undefined, { style: "currency", currency: "EUR", maximumFractionDigits: Number.isInteger(n) ? 0 : 2 }).format(n);
export const capitalized = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
export const when = (iso: string) => new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(iso));
