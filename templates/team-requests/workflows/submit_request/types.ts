// SPDX-License-Identifier: MIT-0
import { z } from "zod";

export const Category = z.enum(["equipment", "software", "travel", "training", "other"]);
export const Status = z.enum(["approved", "declined"]);
