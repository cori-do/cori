import { type CoriConfig, type RunView } from "@cori-do/client";
import { cori } from "./cori";

let config: CoriConfig = {};
/** The hosted `__cori/config.json`: app, baseUrl, auth mode, and whether this is a version's preview. */
export const coriConfig = () => config;

/** Loads the config once before the first render. The dev identity needs nothing here: under `cori up` the client and
 *  the dev widget keep one per app (`cori.tok.<app>`), defaulting to the first `[[dev.personas]]` of cori.toml. */
export async function bootstrap(): Promise<void> {
  config = await cori.config();
}

/** A run that did not succeed becomes an Error carrying the failing step's message (a failed run is HTTP 200). */
export class RunFailed extends Error {
  constructor(public run: RunView<any, any>) {
    super(run.errorDetail?.message ?? run.error ?? `Run ${run.status}`);
    this.name = "RunFailed";
  }
}
export async function must<V, O>(p: Promise<RunView<V, O>>): Promise<RunView<V, O>> {
  const r = await p;
  if (r.status !== "succeeded") throw new RunFailed(r);
  return r;
}

/** One key per form instance: a double submit on an `idempotency: header` endpoint replays the first run. */
export const newIdempotencyKey = () => crypto.randomUUID();

export function formatValue(v: unknown): string {
  if (v === null || v === undefined || v === "") return "—";
  if (typeof v === "number") return new Intl.NumberFormat().format(v);
  if (typeof v === "boolean") return v ? "Yes" : "No";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(formatValue).join(", ");
  return JSON.stringify(v);
}
