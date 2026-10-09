import type { App } from "@modelcontextprotocol/ext-apps";
import type { RequestOptions } from "@modelcontextprotocol/sdk/shared/protocol.js";
import { ContentBlockSchema } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

export const OPENAI_MODEL_CONTEXT_KEY = "openai/modelContext";

const ModelContextUpdateIdSchema = z.string().min(1);

export const OpenAIModelContextMetadataSchema = z.object({
  [OPENAI_MODEL_CONTEXT_KEY]: z.object({
    updateId: ModelContextUpdateIdSchema,
  }),
});

export const OpenAIModelContextHostStateSchema = z.union([
  z.object({
    content: z.array(ContentBlockSchema).optional(),
    structuredContent: z.record(z.string(), z.unknown()).optional(),
    updateId: ModelContextUpdateIdSchema,
  }),
  z.null(),
]);

export type OpenAIModelContextMetadata = z.infer<
  typeof OpenAIModelContextMetadataSchema
>;

export type OpenAIModelContextHostState = z.infer<
  typeof OpenAIModelContextHostStateSchema
>;

type ModelContextParams = Parameters<App["updateModelContext"]>[0];
export type OpenAIModelContextUpdateResult =
  OpenAIModelContextMetadata[typeof OPENAI_MODEL_CONTEXT_KEY];

/** Model context updates exposed for one MCP App instance. */
export type OpenAIModelContext = {
  getCurrent(): OpenAIModelContextHostState | undefined;
  update(
    params: ModelContextParams,
    options?: RequestOptions,
  ): Promise<OpenAIModelContextUpdateResult | undefined>;
};

export function createModelContext(app: App): OpenAIModelContext {
  return {
    getCurrent: () => {
      const hostState = OpenAIModelContextHostStateSchema.safeParse(
        app.getHostContext()?.[OPENAI_MODEL_CONTEXT_KEY],
      );
      return hostState.success ? hostState.data : undefined;
    },
    update: async (params, options) => {
      const result = await app.updateModelContext(params, options);
      const metadata = OpenAIModelContextMetadataSchema.safeParse(result._meta);
      return metadata.success
        ? metadata.data[OPENAI_MODEL_CONTEXT_KEY]
        : undefined;
    },
  };
}
