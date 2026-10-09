import type { App } from "@modelcontextprotocol/ext-apps";
import { z } from "zod";

import { OPENAI_RESOURCE_METADATA_KEY } from "../shared/resources.js";
import { createDeepLink, type OpenAIDeepLink } from "./deep-link.js";
import { createFiles, type OpenAIFiles } from "./files.js";
import {
  createMessage,
  OPENAI_MESSAGE_KEY,
  type OpenAIMessage,
} from "./message.js";
import {
  createModelContext,
  OPENAI_MODEL_CONTEXT_KEY,
  type OpenAIModelContext,
} from "./model-context.js";
import { createResources, type OpenAIResources } from "./resources.js";

const interactionCursorSchema = z.enum(["default", "pointer"]).catch("pointer");

/** OpenAI-specific extension surface for one MCP App instance. */
export class OpenAIExtensions {
  readonly deepLink: OpenAIDeepLink;
  private readonly app: App;
  private readonly fileApi: OpenAIFiles;
  private readonly messageApi: OpenAIMessage;
  private readonly modelContextApi: OpenAIModelContext;
  private readonly resourcesApi: OpenAIResources;

  get files(): OpenAIFiles | undefined {
    return this.app.getHostCapabilities()?.experimental?.["openai/files"]
      ? this.fileApi
      : undefined;
  }

  get message(): OpenAIMessage | undefined {
    const experimental = this.app.getHostCapabilities()?.experimental;
    return experimental?.[OPENAI_MESSAGE_KEY] != null
      ? this.messageApi
      : undefined;
  }

  get modelContext(): OpenAIModelContext | undefined {
    const experimental = this.app.getHostCapabilities()?.experimental;
    return experimental?.[OPENAI_MODEL_CONTEXT_KEY] != null
      ? this.modelContextApi
      : undefined;
  }

  /** Host-managed resources, available once the host advertises support during `app.connect()`. */
  get resources(): OpenAIResources | undefined {
    const experimental: Record<string, unknown> | undefined =
      this.app.getHostCapabilities()?.experimental;
    return experimental?.[OPENAI_RESOURCE_METADATA_KEY] != null
      ? this.resourcesApi
      : undefined;
  }

  constructor(app: App) {
    this.app = app;
    this.deepLink = createDeepLink(app);
    this.fileApi = createFiles(app);
    this.messageApi = createMessage(app);
    this.modelContextApi = createModelContext(app);
    this.resourcesApi = createResources(app);

    if (typeof document !== "undefined") {
      const applyInteractionCursor = (): void => {
        document.documentElement.style.setProperty(
          "--cursor-interaction",
          interactionCursorSchema.parse(
            app.getHostContext()?.["openai/interactionCursor"],
          ),
        );
      };
      app.addEventListener("hostcontextchanged", applyInteractionCursor);
      // Initial host context does not emit a hostcontextchanged event.
      const connect = app.connect.bind(app);
      app.connect = async (...args) => {
        await connect(...args);
        applyInteractionCursor();
      };
      applyInteractionCursor();
    }
  }
}
