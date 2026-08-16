// Workflow parameter form fields, shared by the launcher's workflow
// pane (run-time input) and the schedule modals (create-time input —
// a schedule fires unattended, so its input is captured up front).

import type { ParameterDef } from "../lib/api";

export function ParamField({
  param,
  value,
  onChange,
}: {
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  const id = `p-${param.name}`;
  return (
    <div className="param-row">
      <label htmlFor={id} className="param-name">
        {param.name}
        {param.required && <span className="param-required"> *</span>}
        <span className="param-type">{param.type}</span>
      </label>
      {param.description && (
        <div className="param-desc">{param.description}</div>
      )}
      <ParamInput id={id} param={param} value={value} onChange={onChange} />
    </div>
  );
}

function ParamInput({
  id,
  param,
  value,
  onChange,
}: {
  id: string;
  param: ParameterDef;
  value: unknown;
  onChange: (v: unknown) => void;
}) {
  if (param.type === "boolean") {
    return (
      <input
        id={id}
        type="checkbox"
        checked={value === true}
        onChange={(e) => onChange(e.target.checked)}
      />
    );
  }

  if (param.type === "enum" && Array.isArray(param.values)) {
    return (
      <select
        id={id}
        className="param-input"
        value={value == null ? "" : String(value)}
        onChange={(e) => onChange(e.target.value)}
      >
        <option value="">— select —</option>
        {param.values.map((v, i) => (
          <option key={i} value={String(v)}>
            {String(v)}
          </option>
        ))}
      </select>
    );
  }

  if (param.type === "number") {
    return (
      <input
        id={id}
        className="param-input"
        type="number"
        value={value == null ? "" : String(value)}
        min={param.min ?? undefined}
        max={param.max ?? undefined}
        onChange={(e) =>
          onChange(e.target.value === "" ? null : Number(e.target.value))
        }
      />
    );
  }

  return (
    <input
      id={id}
      className="param-input"
      type="text"
      value={value == null ? "" : String(value)}
      onChange={(e) => onChange(e.target.value)}
      placeholder={param.type === "path" ? "/abs/path or ./relative" : ""}
    />
  );
}

/** Defaults declared in the manifest, as an initial values map. */
export function paramDefaults(
  params: ParameterDef[],
): Record<string, unknown> {
  const defaults: Record<string, unknown> = {};
  for (const p of params) {
    if (p.default !== undefined && p.default !== null) {
      defaults[p.name] = p.default;
    }
  }
  return defaults;
}

/** Values the form considers "not provided" (dropped from the input). */
export function isBlank(v: unknown): boolean {
  return v === undefined || v === null || v === "";
}

/** Required params (without a manifest default) still missing a value. */
export function missingRequired(
  params: ParameterDef[],
  values: Record<string, unknown>,
): string[] {
  return params
    .filter((p) => p.required && p.default == null && isBlank(values[p.name]))
    .map((p) => p.name);
}
