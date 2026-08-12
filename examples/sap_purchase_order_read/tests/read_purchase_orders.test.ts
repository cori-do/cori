import readPurchaseOrders from "../steps/01_read_purchase_orders.ts";
import fixture from "./fixtures/purchase_orders.json" with { type: "json" };
import { assert, assertEquals } from "./assert.ts";

Deno.test("builds a bounded, argument-safe list command", () => {
  const command = readPurchaseOrders.command({
    operation: "list",
    max_results: 25,
  });

  assertEquals(command, [
    "cori-sap",
    "purchase-orders",
    "list",
    "--limit",
    "25",
  ]);
});

Deno.test("rejects a get command without a purchase-order ID", () => {
  let rejected = false;
  try {
    readPurchaseOrders.command({
      operation: "get",
      max_results: 25,
    });
  } catch {
    rejected = true;
  }
  assert(rejected, "get must fail before spawn when its ID is absent");
});

Deno.test("normalizes the adapter response into workflow output", async () => {
  assert(readPurchaseOrders.parse, "CLI step must declare a parser");
  const output = await readPurchaseOrders.parse!(JSON.stringify(fixture), {
    stderr: "",
    exitCode: 0,
  });

  assertEquals(output, {
    purchase_orders: fixture.purchase_orders,
    purchase_order_count: 1,
    result_limit: 25,
    has_more: false,
  });
});

Deno.test("normalizes a get response into the same workflow output", async () => {
  assert(readPurchaseOrders.parse, "CLI step must declare a parser");
  const output = await readPurchaseOrders.parse!(
    JSON.stringify({ purchase_order: fixture.purchase_orders[0] }),
    { stderr: "", exitCode: 0 },
  );

  assertEquals(output, {
    purchase_orders: fixture.purchase_orders,
    purchase_order_count: 1,
    result_limit: 1,
    has_more: false,
  });
});
