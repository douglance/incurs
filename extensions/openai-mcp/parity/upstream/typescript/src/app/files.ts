import type { App } from "@modelcontextprotocol/ext-apps";
import {
  EmptyResultSchema,
  type EmptyResult,
} from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

export const OPENAI_FILE_OPEN_METHOD = "openai/files/open";

export const OpenAIFileOpenParamsSchema = z.strictObject({
  path: z.string().min(1),
});

export type OpenAIFileOpenParams = z.infer<typeof OpenAIFileOpenParamsSchema>;

/** Opens a file in the host's native file viewer or editor. */
export type OpenAIFiles = {
  open(path: string): Promise<EmptyResult>;
};

type OpenAIFileProtocol = {
  request(
    request: {
      method: typeof OPENAI_FILE_OPEN_METHOD;
      params: OpenAIFileOpenParams;
    },
    resultSchema: typeof EmptyResultSchema,
  ): Promise<EmptyResult>;
};

export function createFiles(app: App): OpenAIFiles {
  const protocol = app as unknown as OpenAIFileProtocol;
  return {
    open: (path) =>
      protocol.request(
        {
          method: OPENAI_FILE_OPEN_METHOD,
          params: OpenAIFileOpenParamsSchema.parse({ path }),
        },
        EmptyResultSchema,
      ),
  };
}
