import { z } from "zod";

import { OPENAI_RESOURCE_METADATA_KEY } from "../shared/resources.js";

/** Host-owned resource context carried on tool calls. */
export const OpenAIResourceToolCallMetadataSchema = z.object({
  [OPENAI_RESOURCE_METADATA_KEY]: z
    .object({
      path: z
        .string()
        .describe("Path in the MCP server host filesystem namespace."),
    })
    .optional(),
});

export type OpenAIResourceToolCallMetadata = z.infer<
  typeof OpenAIResourceToolCallMetadataSchema
>;

/** Reads the host-owned file path from tool-call metadata. Throws on malformed metadata. */
export function getResourcePath(meta: unknown): string | undefined {
  return OpenAIResourceToolCallMetadataSchema.parse(meta ?? {})[
    OPENAI_RESOURCE_METADATA_KEY
  ]?.path;
}
