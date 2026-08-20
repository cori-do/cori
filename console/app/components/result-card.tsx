import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  ResolvedResult,
  ResolvedResultField,
  ResolvedResultSection,
} from "../lib/api";
import { formatDuration } from "../lib/format";
import { JsonBlock, Markdown, RichText } from "./rich-content";

export function ResultCard({
  result,
  partial = false,
  compact = false,
}: {
  result: ResolvedResult;
  partial?: boolean;
  compact?: boolean;
}) {
  const [openError, setOpenError] = useState<string | null>(null);
  const fields = result.fields ?? [];
  const artifacts = result.artifacts ?? [];
  const sections = result.sections ?? [];
  const issues = result.issues ?? [];

  async function openArtifact(url: string) {
    setOpenError(null);
    try {
      await openUrl(url);
    } catch (error) {
      setOpenError(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <section className={`result-card${compact ? " is-compact" : ""}`}>
      <div className="result-card-kicker">{partial ? "Partial result" : "Result"}</div>
      <h2 className="result-card-headline">
        {result.headline || "Result headline unavailable"}
      </h2>
      {result.description && (
        <div className="result-card-description">
          <Markdown source={result.description} />
        </div>
      )}

      {fields.length > 0 && !compact && (
        <div className="result-metrics">
          {fields.map((field, index) => (
            <div
              key={`${field.label}:${index}`}
              className={`result-metric tone-${field.tone}`}
            >
              <span className="result-metric-label">{field.label}</span>
              <strong className="result-metric-value">{formatField(field)}</strong>
            </div>
          ))}
        </div>
      )}

      {artifacts.length > 0 && (
        <div className="result-artifacts">
          {artifacts.map((artifact, index) => (
            <button
              key={`${artifact.url}:${index}`}
              type="button"
              className="btn result-artifact"
              onClick={() => void openArtifact(artifact.url)}
              title={artifact.url}
            >
              <span>{artifact.label}</span>
              <span aria-hidden>Open ↗</span>
            </button>
          ))}
        </div>
      )}
      {openError && (
        <p className="result-open-error" role="alert">
          Could not open artifact: {openError}
        </p>
      )}

      {!compact && sections.length > 0 && (
        <div className="result-sections">
          {sections.map((section, index) => (
            <ResultSectionView key={`${section.label}:${index}`} section={section} />
          ))}
        </div>
      )}

      {!compact && issues.length > 0 && (
        <details className="result-issues">
          <summary>{issues.length} result issue{issues.length === 1 ? "" : "s"}</summary>
          <ul>
            {issues.map((issue, index) => (
              <li key={`${issue.item}:${index}`}>
                <code>{issue.item}</code>: {issue.message}
              </li>
            ))}
          </ul>
        </details>
      )}

      {!compact && (
        <details className="result-raw">
          <summary>Raw result JSON</summary>
          <JsonBlock value={result} />
        </details>
      )}
    </section>
  );
}

function formatField(field: ResolvedResultField): string {
  const value = typeof field.value === "number" ? field.value : Number.NaN;
  switch (field.format) {
    case "number":
      return Number.isFinite(value)
        ? new Intl.NumberFormat().format(value)
        : inlineValue(field.value);
    case "currency":
      return Number.isFinite(value) && field.currency
        ? new Intl.NumberFormat(undefined, {
            style: "currency",
            currency: field.currency,
          }).format(value)
        : inlineValue(field.value);
    case "percent":
      return Number.isFinite(value)
        ? `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 }).format(value)}%`
        : inlineValue(field.value);
    case "duration":
      return Number.isFinite(value) ? formatDuration(value) : inlineValue(field.value);
    case "auto":
      return inlineValue(field.value);
  }
}

function ResultSectionView({ section }: { section: ResolvedResultSection }) {
  const display = effectiveDisplay(section);
  return (
    <section className="result-section">
      <h3>{section.label}</h3>
      {display === "text" && (
        <div className="result-prose">
          <RichText value={String(section.value)} />
        </div>
      )}
      {display === "list" && <ResultList value={section.value} />}
      {display === "table" && <ResultTable value={section.value} />}
      {display === "key_value" && <ResultKeyValue value={section.value} />}
      {display === "tree" && (
        <details className="result-tree">
          <summary>Show structured data</summary>
          <JsonBlock value={section.value} />
        </details>
      )}
    </section>
  );
}

type EffectiveDisplay = "text" | "list" | "table" | "key_value" | "tree";

function effectiveDisplay(section: ResolvedResultSection): EffectiveDisplay {
  if (section.display !== "auto") {
    return section.display === "text" ? "text" : section.display;
  }
  const value = section.value;
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return "text";
  }
  if (Array.isArray(value)) {
    if (value.every(isScalar)) return "list";
    if (value.every((row) => isRecord(row) || Array.isArray(row))) return "table";
    return "tree";
  }
  if (isRecord(value) && Object.values(value).every(isScalar)) return "key_value";
  return "tree";
}

function ResultList({ value }: { value: unknown }) {
  const items = Array.isArray(value) ? value : [];
  return (
    <ul className="result-list">
      {items.map((item, index) => (
        <li key={index}>{inlineValue(item)}</li>
      ))}
    </ul>
  );
}

function ResultKeyValue({ value }: { value: unknown }) {
  if (!isRecord(value)) return null;
  return (
    <dl className="result-kv">
      {Object.entries(value).map(([key, item]) => (
        <div key={key}>
          <dt>{key}</dt>
          <dd>{inlineValue(item)}</dd>
        </div>
      ))}
    </dl>
  );
}

function ResultTable({ value }: { value: unknown }) {
  const [expanded, setExpanded] = useState(false);
  const rows = Array.isArray(value) ? value : [];
  const visible = expanded ? rows : rows.slice(0, 10);

  if (rows.every(isRecord)) {
    const columns = [...new Set(rows.flatMap((row) => Object.keys(row)))];
    return (
      <div>
        <div className="result-table-wrap">
          <table className="result-table">
            <thead>
              <tr>{columns.map((column) => <th key={column}>{column}</th>)}</tr>
            </thead>
            <tbody>
              {visible.map((row, index) => (
                <tr key={index}>
                  {columns.map((column) => <td key={column}>{inlineValue(row[column])}</td>)}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <TableExpansion rows={rows.length} expanded={expanded} setExpanded={setExpanded} />
      </div>
    );
  }

  if (rows.every(Array.isArray)) {
    return (
      <div>
        <div className="result-table-wrap">
          <table className="result-table">
            <tbody>
              {visible.map((row, index) => (
                <tr key={index}>
                  {(row as unknown[]).map((cell: unknown, cellIndex: number) => (
                    <td key={cellIndex}>{inlineValue(cell)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <TableExpansion rows={rows.length} expanded={expanded} setExpanded={setExpanded} />
      </div>
    );
  }

  return (
    <details className="result-tree">
      <summary>Show structured data</summary>
      <JsonBlock value={value} />
    </details>
  );
}

function TableExpansion({
  rows,
  expanded,
  setExpanded,
}: {
  rows: number;
  expanded: boolean;
  setExpanded: (expanded: boolean) => void;
}) {
  if (rows <= 10) return null;
  return (
    <button type="button" className="result-expand" onClick={() => setExpanded(!expanded)}>
      {expanded ? "Show first 10 rows" : `Show ${rows - 10} more rows`}
    </button>
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isScalar(value: unknown): boolean {
  return value == null || ["string", "number", "boolean"].includes(typeof value);
}

function inlineValue(value: unknown): string {
  if (value == null) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}
