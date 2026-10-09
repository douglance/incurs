import { z } from "zod";

import { NonBlankStringSchema } from "./strings.js";

/** Arguments injected when an MCP App opens through a file entrypoint. */
export const OpenAIFileEntrypointInputSchema = z.object({
  file: z.object({
    name: z.string().min(1),
    resourceUri: NonBlankStringSchema,
  }),
});

export type OpenAIFileEntrypointInput = z.infer<
  typeof OpenAIFileEntrypointInputSchema
>;
