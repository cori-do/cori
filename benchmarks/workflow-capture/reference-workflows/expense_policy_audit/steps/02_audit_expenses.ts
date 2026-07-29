import { step } from "@cori-do/sdk";
import { z } from "zod";
const Input = z.object({ values: z.array(z.array(z.string())), run_tag: z.string() });
const Output = z.object({
  rows: z.array(z.array(z.string())),
  exception_count: z.number().int(),
  reason_codes: z.string(),
  expense_draft_summary: z.string(),
});
export default step.code({
  description: "Apply deterministic expense policy checks",
  input: Input,
  output: Output,
  run: ({ values, run_tag }) => {
    const source = values.slice(1).map((row) => ({
      expenseId: row[0] ?? "",
      category: row[1] ?? "",
      amount: Number(row[2] ?? ""),
      receipt: (row[3] ?? "").toLowerCase() === "true",
      nights: Number(row[4] ?? ""),
      attendees: Number(row[5] ?? ""),
      personal: (row[6] ?? "").toLowerCase() === "true",
      invoiceId: row[7] ?? "",
    }));
    const invoiceCounts = new Map<string, number>();
    for (const expense of source) {
      invoiceCounts.set(
        expense.invoiceId,
        (invoiceCounts.get(expense.invoiceId) ?? 0) + 1,
      );
    }
    const assessed = source.map((expense) => {
      const reasons: string[] = [];
      if (!expense.receipt && expense.amount >= 75) reasons.push("missing_receipt");
      if (
        expense.category === "hotel" &&
        expense.nights > 0 &&
        expense.amount / expense.nights > 250
      ) {
        reasons.push("hotel_rate");
      }
      if (
        expense.category === "meal" &&
        expense.attendees > 0 &&
        expense.amount / expense.attendees > 60
      ) {
        reasons.push("meal_per_person");
      }
      if (expense.personal) reasons.push("personal");
      if ((invoiceCounts.get(expense.invoiceId) ?? 0) > 1) {
        reasons.push("duplicate_invoice");
      }
      return { ...expense, reasons };
    }).sort((left, right) => left.expenseId.localeCompare(right.expenseId));
    const failures = assessed.filter((expense) => expense.reasons.length > 0);
    const reason_codes = [...new Set(failures.flatMap((expense) => expense.reasons))]
      .join(", ");
    return {
      rows: assessed.map((expense) => [
        expense.expenseId,
        expense.reasons.length === 0 ? "PASS" : "FAIL",
        expense.reasons.join(";"),
        run_tag,
      ]),
      exception_count: failures.length,
      reason_codes,
      expense_draft_summary: `${run_tag}\nExceptions: ${failures.length}\nReasons: ${reason_codes || "none"}`,
    };
  },
});
