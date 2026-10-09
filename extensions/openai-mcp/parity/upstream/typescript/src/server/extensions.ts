import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

import {
  createElicitInput,
  type OpenAIElicitInput,
} from "./forms/elicitation.js";
import { createMentions, type OpenAIMentions } from "./mentions.js";
import { createSettings, type OpenAISettings } from "./settings.js";

/** Adds OpenAI-specific extensions to one MCP server. */
export class OpenAIExtensions {
  readonly elicitInput: OpenAIElicitInput;
  readonly mentions: OpenAIMentions;
  readonly settings: OpenAISettings;

  constructor(server: McpServer) {
    this.elicitInput = createElicitInput(server);
    this.mentions = createMentions(server);
    this.settings = createSettings(server);
  }
}
