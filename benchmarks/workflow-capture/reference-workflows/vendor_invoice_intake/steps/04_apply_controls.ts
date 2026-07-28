import { step } from "@cori-do/sdk";
import { z } from "zod";

const Invoice = z.object({
  id: z.string(),
  vendor: z.string(),
  invoice_number: z.string(),
  currency: z.string(),
  net: z.number(),
  tax: z.number(),
  gross: z.number(),
  due_date: z.string(),
});
const Input = z.object({ invoices: z.array(Invoice), as_of: z.string() });
const Output = z.object({
  rows: z.array(z.array(z.string())),
  blocked: z.array(z.string()),
  counts: z.object({ blocked: z.number(), overdue: z.number(), payable: z.number() }),
});

/** The accounts payable controls are policy, so they stay deterministic. */
export default step.code({
  description: "Apply the payable controls and order the register",
  input: Input,
  output: Output,
  run: ({ invoices, as_of }) => {
    const asOfDate = as_of.slice(0, 10);
    const counts = { blocked: 0, overdue: 0, payable: 0 };
    const assessed = invoices.map((invoice) => {
      const balances = Math.abs(invoice.net + invoice.tax - invoice.gross) < 0.005;
      const status = !balances
        ? "blocked"
        : invoice.due_date < asOfDate
        ? "overdue"
        : "payable";
      counts[status as keyof typeof counts] += 1;
      return { ...invoice, status };
    }).sort((left, right) => {
      const rank = { blocked: 0, overdue: 1, payable: 2 } as const;
      return rank[left.status as keyof typeof rank] - rank[right.status as keyof typeof rank] ||
        left.due_date.localeCompare(right.due_date) ||
        left.invoice_number.localeCompare(right.invoice_number);
    });
    return {
      rows: assessed.map((invoice) => [
        invoice.id,
        invoice.vendor,
        invoice.invoice_number,
        invoice.currency.toUpperCase(),
        invoice.net.toFixed(2),
        invoice.tax.toFixed(2),
        invoice.gross.toFixed(2),
        invoice.due_date,
        invoice.status,
      ]),
      blocked: assessed.filter((invoice) => invoice.status === "blocked").map((invoice) => invoice.invoice_number),
      counts,
    };
  },
});
