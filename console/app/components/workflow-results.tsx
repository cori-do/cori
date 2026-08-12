import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { RunTrace } from "../lib/api";

const MAX_TABLE_ROWS = 100;
type ResultScalar = string | number | boolean | null;

export function resultsFromTrace(trace: RunTrace): unknown | null {
  if (trace.status !== "succeeded") return null;
  for (let index = trace.activities.length - 1; index >= 0; index -= 1) {
    const activity = trace.activities[index];
    if (activity?.status !== "succeeded" || !isRecord(activity.output)) {
      continue;
    }
    if (Object.hasOwn(activity.output, "results")) {
      return activity.output.results;
    }
  }
  return null;
}

export function WorkflowResults({ results }: { results: unknown }) {
  const record = isRecord(results) ? results : null;
  const title = typeof record?.title === "string"
    ? record.title
    : "Workflow results";
  const summary = typeof record?.summary === "string" ? record.summary : null;
  const body = record
    ? Object.fromEntries(
      Object.entries(record).filter(([key]) =>
        key !== "title" && key !== "summary"
      ),
    )
    : results;

  return (
    <section className="workflow-results" aria-label="Workflow results">
      <header className="workflow-results-head">
        <span className="workflow-results-kicker">Results</span>
        <h2>{title}</h2>
        {summary && <p>{summary}</p>}
      </header>
      {!isEmptyRecord(body) && (
        <div className="workflow-results-body">
          <ResultValue value={body} />
        </div>
      )}
    </section>
  );
}

function ResultValue({ value, field }: { value: unknown; field?: string }) {
  if (isScalar(value)) {
    return (
      <span className="workflow-result-value">
        {formatScalar(value, field)}
      </span>
    );
  }
  if (Array.isArray(value)) return <ResultArray values={value} field={field} />;
  if (isRecord(value)) return <ResultObject value={value} />;
  return <span className="workflow-result-value">{String(value)}</span>;
}

function ResultObject({ value }: { value: Record<string, unknown> }) {
  const scalarEntries = Object.entries(value).filter(([, entry]) =>
    isScalar(entry)
  );
  const richEntries = Object.entries(value).filter(([, entry]) =>
    !isScalar(entry)
  );
  return (
    <div className="workflow-result-object">
      {scalarEntries.length > 0 && (
        <dl className="workflow-result-facts">
          {scalarEntries.map(([key, entry]) => (
            <div key={key}>
              <dt>{friendlyLabel(key)}</dt>
              <dd>
                <ResultValue value={entry} field={key} />
              </dd>
            </div>
          ))}
        </dl>
      )}
      {richEntries.map(([key, entry]) => (
        <section className="workflow-result-group" key={key}>
          <h3>{friendlyLabel(key)}</h3>
          <ResultValue value={entry} field={key} />
        </section>
      ))}
    </div>
  );
}

function ResultArray({ values, field }: { values: unknown[]; field?: string }) {
  if (values.length === 0) {
    return <span className="workflow-result-empty">None</span>;
  }
  if (values.every(isScalar)) {
    return (
      <ul className="workflow-result-list">
        {values.map((value, index) => (
          <li key={index}>{formatScalar(value, field)}</li>
        ))}
      </ul>
    );
  }
  if (isTabular(values)) return <ResultTable rows={values} />;
  return (
    <div className="workflow-result-cards">
      {values.map((value, index) => (
        <div className="workflow-result-card" key={index}>
          <ResultValue value={value} field={field} />
        </div>
      ))}
    </div>
  );
}

function ResultTable({ rows }: { rows: Array<Record<string, ResultScalar>> }) {
  const columns = Array.from(new Set(rows.flatMap((row) => Object.keys(row))));
  const visibleRows = rows.slice(0, MAX_TABLE_ROWS);
  return (
    <div className="workflow-result-table-wrap">
      <table className="workflow-result-table">
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column}>{friendlyLabel(column)}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {visibleRows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {columns.map((column) => (
                <td key={column}>
                  {formatScalar(row[column] ?? null, column)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length > MAX_TABLE_ROWS && (
        <div className="workflow-result-overflow">
          Showing {MAX_TABLE_ROWS} of {rows.length} rows
        </div>
      )}
    </div>
  );
}

function isTabular(
  values: unknown[],
): values is Array<Record<string, ResultScalar>> {
  return values.every((value) =>
    isRecord(value) && Object.values(value).every(isScalar)
  );
}

function isScalar(value: unknown): value is ResultScalar {
  return value === null ||
    ["string", "number", "boolean"].includes(typeof value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isEmptyRecord(value: unknown): boolean {
  return isRecord(value) && Object.keys(value).length === 0;
}

function friendlyLabel(value: string): string {
  const spaced = value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .trim();
  return spaced.length === 0
    ? value
    : `${spaced[0]?.toUpperCase() ?? ""}${spaced.slice(1)}`;
}

function formatScalar(
  value: ResultScalar,
  field?: string,
): ReactNode {
  if (value === null || value === "") {
    return <span className="is-muted">—</span>;
  }
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number") {
    if (field?.endsWith("_bytes")) return formatBytes(value);
    if (field?.endsWith("_percent")) return `${value}%`;
    return new Intl.NumberFormat().format(value);
  }
  if (/^https?:\/\//.test(value)) {
    return (
      <button
        type="button"
        className="workflow-result-link"
        title={`Open ${value}`}
        onClick={() => void openUrl(value)}
      >
        {value}
      </button>
    );
  }
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(value)) {
    const date = new Date(value);
    if (!Number.isNaN(date.valueOf())) {
      return date.toLocaleString([], {
        dateStyle: "medium",
        timeStyle: "short",
      });
    }
  }
  if (
    field === "status" || field === "response" || field === "major_dimension"
  ) {
    return friendlyLabel(value.toLowerCase());
  }
  return value;
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let amount = value;
  let unit = "B";
  for (const candidate of units) {
    amount /= 1024;
    unit = candidate;
    if (amount < 1024) break;
  }
  return `${
    new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(
      amount,
    )
  } ${unit}`;
}
