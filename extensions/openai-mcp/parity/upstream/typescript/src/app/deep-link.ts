import type { App } from "@modelcontextprotocol/ext-apps";
import { z } from "zod";

export const OPENAI_DEEP_LINK_KEY = "openai/deepLink";

export const OpenAIDeepLinkHostStateSchema = z.object({
  url: z.string(),
});

export type OpenAIDeepLinkHostState = z.infer<
  typeof OpenAIDeepLinkHostStateSchema
>;

// Installed apps and hosts update independently. Normalize older host payloads.
// TODO(victor): Drop the path/query fallback once supported hosts provide url.
const deepLinkHostStateSchema = z.union([
  OpenAIDeepLinkHostStateSchema,
  z
    .object({
      path: z.array(z.string()),
      query: z.array(z.tuple([z.string(), z.string()])),
    })
    .transform(({ path, query }) => {
      const search = new URLSearchParams(query).toString();
      return {
        url: `/${path.map(encodeURIComponent).join("/")}${search ? `?${search}` : ""}`,
      };
    }),
]);

/** Deep-link activations exposed for one MCP App instance. */
export type OpenAIDeepLink = {
  getCurrent(): OpenAIDeepLinkHostState | undefined;
};

export function createDeepLink(app: App): OpenAIDeepLink {
  return {
    getCurrent: () => {
      const hostState = deepLinkHostStateSchema.safeParse(
        app.getHostContext()?.[OPENAI_DEEP_LINK_KEY],
      );
      return hostState.success ? hostState.data : undefined;
    },
  };
}
