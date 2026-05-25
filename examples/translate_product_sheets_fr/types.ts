import { z } from "zod";

export const SourceRow = z.object({
  sku: z.string(),
  upc: z.string().optional(),
  name_en: z.string(),
  description_en: z.string(),
  material_en: z.string().nullable(),
  safety_info_en: z.string().nullable(),
  operator_contact: z.string().nullable(),
  weight_g: z.number().nullable(),
  price_eur: z.number(),
});
export type SourceRow = z.infer<typeof SourceRow>;

export const TranslatedRow = SourceRow.omit({
  name_en: true,
  description_en: true,
  material_en: true,
  safety_info_en: true,
}).extend({
  name_fr: z.string(),
  description_fr: z.string(),
  material_fr: z.string().nullable(),
  safety_info_fr: z.string().nullable(),
});
export type TranslatedRow = z.infer<typeof TranslatedRow>;

export const GpsrCheck = z.object({
  sku: z.string(),
  check: z.enum(["OK", "NOK"]),
  invalid_reason: z.string().nullable(),
});
export type GpsrCheck = z.infer<typeof GpsrCheck>;
