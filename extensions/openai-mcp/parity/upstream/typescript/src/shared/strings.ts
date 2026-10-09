import { z } from "zod";

export const NonBlankStringSchema = z.string().regex(/\S/);
