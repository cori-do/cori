// SPDX-License-Identifier: MIT-0
import { z } from "zod";

export const Ticket = z.object({
  id: z.string(),
  subject: z.string(),
  body: z.string(),
  customer_id: z.string(),
});
export const Category = z.enum(["billing", "bug", "feature", "other"]);
export const Priority = z.enum(["P1", "P2", "P3"]);
export const Plan = z.enum(["free", "pro", "enterprise"]);
