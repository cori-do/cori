import { step } from "@cori-do/sdk";
import { z } from "zod";

const Input = z.object({
  operation: z.enum(["get", "list"]),
  purchase_order_id: z.string().trim().min(1).optional(),
  max_results: z.number().int().min(1).max(100),
});

const PurchaseOrder = z.object({
  purchase_order: z.string().min(1),
  purchase_order_type: z.string().optional(),
  company_code: z.string().optional(),
  purchasing_organization: z.string().optional(),
  purchasing_group: z.string().optional(),
  supplier: z.string().optional(),
  document_currency: z.string().optional(),
  purchase_order_date: z.string().optional(),
  creation_date: z.string().optional(),
  last_change_date_time: z.string().optional(),
  processing_status: z.string().optional(),
  language: z.string().optional(),
  payment_terms: z.string().optional(),
  deletion_code: z.string().optional(),
});

const GetResponse = z.object({
  purchase_order: PurchaseOrder,
});

const ListResponse = z.object({
  purchase_orders: z.array(PurchaseOrder),
  count: z.number().int().nonnegative(),
  limit: z.number().int().min(1).max(100),
  has_more: z.boolean(),
});

const Output = z.object({
  purchase_orders: z.array(PurchaseOrder),
  purchase_order_count: z.number().int().nonnegative(),
  result_limit: z.number().int().min(1).max(100),
  has_more: z.boolean(),
});

export default step.cli({
  description: "Read purchase orders from SAP S/4HANA",
  input: Input,
  output: Output,
  retries: { max: 1, backoff: "linear" },
  timeout_ms: 60000,
  command: ({
    operation,
    purchase_order_id,
    max_results,
  }) => {
    const args = ["purchase-orders", operation];

    if (operation === "get") {
      if (!purchase_order_id) {
        throw new Error(
          "purchase_order_id is required when operation is get",
        );
      }
      args.push("--id", purchase_order_id);
    } else {
      args.push("--limit", String(max_results));
    }

    return ["cori-sap", ...args];
  },
  parse: (stdout) => {
    const raw: unknown = JSON.parse(stdout);
    const list = ListResponse.safeParse(raw);
    if (list.success) {
      if (list.data.count !== list.data.purchase_orders.length) {
        throw new Error("cori-sap list count does not match its records");
      }
      return {
        purchase_orders: list.data.purchase_orders,
        purchase_order_count: list.data.count,
        result_limit: list.data.limit,
        has_more: list.data.has_more,
      };
    }

    const get = GetResponse.parse(raw);
    return {
      purchase_orders: [get.purchase_order],
      purchase_order_count: 1,
      result_limit: 1,
      has_more: false,
    };
  },
});
