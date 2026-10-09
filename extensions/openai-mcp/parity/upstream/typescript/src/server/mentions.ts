import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { RequestHandlerExtra } from "@modelcontextprotocol/sdk/shared/protocol.js";
import {
  IconSchema,
  ResourceLinkSchema,
  type ServerNotification,
  type ServerRequest,
} from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

import { NonBlankStringSchema } from "../shared/strings.js";

export const OpenAIMentionResourceSchema = z.strictObject({
  icons: z.array(IconSchema).optional(),
  resourceUri: NonBlankStringSchema,
  subtitle: NonBlankStringSchema.optional(),
  title: NonBlankStringSchema,
  type: z.literal("resource"),
});
export const OpenAIMentionItemSchema = z.discriminatedUnion("type", [
  ResourceLinkSchema,
  OpenAIMentionResourceSchema,
]);
export const OpenAIMentionSearchParamsSchema = z.object({
  query: z.string(),
});
export const OpenAIMentionSearchResultSchema = z.strictObject({
  items: z.array(OpenAIMentionItemSchema),
});

/** One MCP resource returned from mention search. */
export type OpenAIMentionResource = z.infer<typeof OpenAIMentionResourceSchema>;
/** One result returned from mention search. */
export type OpenAIMentionItem = z.infer<typeof OpenAIMentionItemSchema>;
/** Request payload for Codex mention search. */
export type OpenAIMentionSearchParams = z.infer<
  typeof OpenAIMentionSearchParamsSchema
>;
/** Response payload for Codex mention search. */
export type OpenAIMentionSearchResult = z.infer<
  typeof OpenAIMentionSearchResultSchema
>;
export type OpenAIMentionSearchHandler = (
  params: OpenAIMentionSearchParams,
  extra: RequestHandlerExtra<ServerRequest, ServerNotification>,
) => OpenAIMentionSearchResult | Promise<OpenAIMentionSearchResult>;
/** Mention-search extensions exposed for one MCP server instance. */
export type OpenAIMentions = {
  setHandler(handler: OpenAIMentionSearchHandler): void;
};

export function createMentions(
  server: Pick<McpServer, "registerTool">,
): OpenAIMentions {
  let mentionSearchHandler: OpenAIMentionSearchHandler | null = null;
  let mentionsRegistered = false;

  return {
    setHandler: (handler) => {
      if (!mentionsRegistered) {
        server.registerTool(
          "search_mentions",
          {
            annotations: { readOnlyHint: true },
            inputSchema: OpenAIMentionSearchParamsSchema,
            outputSchema: OpenAIMentionSearchResultSchema,
            _meta: {
              "openai/extensions": { "mentions/search": {} },
              ui: { visibility: ["app"] },
            },
          },
          async (params, extra) => ({
            content: [],
            structuredContent:
              mentionSearchHandler == null
                ? { items: [] }
                : await mentionSearchHandler(params, extra),
          }),
        );
        mentionsRegistered = true;
      }
      mentionSearchHandler = handler;
    },
  };
}
