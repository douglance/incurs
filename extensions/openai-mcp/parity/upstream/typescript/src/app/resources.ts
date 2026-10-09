import type { App } from "@modelcontextprotocol/ext-apps";
import type { RequestOptions } from "@modelcontextprotocol/sdk/shared/protocol.js";
import {
  EmptyResultSchema,
  ResourceUpdatedNotificationSchema,
  type EmptyResult,
  type ReadResourceRequest,
  type ReadResourceResult,
  type ResourceUpdatedNotification,
  type SubscribeRequest,
  type UnsubscribeRequest,
} from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

import { OPENAI_RESOURCE_METADATA_KEY } from "../shared/resources.js";
import { NonBlankStringSchema } from "../shared/strings.js";

export const OPENAI_MCP_APP_RESOURCE_WRITE_METHOD = "openai/resources/write";

const ResourceWriteBaseSchema = {
  ifMatch: z.string().min(1).optional(),
  uri: NonBlankStringSchema,
};
const BlobResourceWriteSchema = z.strictObject({
  ...ResourceWriteBaseSchema,
  blob: z.base64(),
  text: z.never().optional(),
});
const TextResourceWriteSchema = z.strictObject({
  ...ResourceWriteBaseSchema,
  blob: z.never().optional(),
  text: z.string(),
});

export const OpenAIResourceRepresentationSchema = z.enum(["blob", "text"]);
export const OpenAIResourceReadMetadataSchema = z.object({
  [OPENAI_RESOURCE_METADATA_KEY]: z
    .object({
      representation: OpenAIResourceRepresentationSchema.optional(),
    })
    .optional(),
});
export const OpenAIResourceMetadataSchema = z.object({
  etag: z.string().optional(),
  writable: z.boolean().optional(),
});
export const OpenAIResourceContentMetadataSchema = z.object({
  [OPENAI_RESOURCE_METADATA_KEY]: OpenAIResourceMetadataSchema.optional(),
});
export const OpenAIResourceWriteParamsSchema = z.union([
  BlobResourceWriteSchema,
  TextResourceWriteSchema,
]);
export const OpenAIResourceWriteResultSchema = z.discriminatedUnion("outcome", [
  z.object({
    etag: z.string(),
    outcome: z.enum(["conflict", "saved"]),
  }),
  z.object({
    maxBytes: z.number(),
    outcome: z.literal("too-large"),
  }),
]);

/** Representation requested when reading a host-managed resource. */
export type OpenAIResourceRepresentation = z.infer<
  typeof OpenAIResourceRepresentationSchema
>;

/** OpenAI metadata carried in `params._meta` on MCP `resources/read` requests. Omit `representation` to auto-detect UTF-8 text or a base64 blob. */
export type OpenAIResourceReadMetadata = z.infer<
  typeof OpenAIResourceReadMetadataSchema
>;

/** OpenAI metadata attached to one host-managed resource content item. */
export type OpenAIResourceMetadata = z.infer<
  typeof OpenAIResourceMetadataSchema
>;

/** OpenAI metadata returned on host-managed MCP resource contents. */
export type OpenAIResourceContentMetadata = z.infer<
  typeof OpenAIResourceContentMetadataSchema
>;

/** Resource content with parsed OpenAI metadata. */
export type OpenAIResourceContent = ReadResourceResult["contents"][number] & {
  openaiMetadata?: OpenAIResourceMetadata;
};

/** MCP resource read result with parsed OpenAI metadata on each content item. */
export type OpenAIResourceReadResult = Omit<ReadResourceResult, "contents"> & {
  contents: Array<OpenAIResourceContent>;
};

/** Options for writing a host-managed resource. Omit `ifMatch` to overwrite the current contents. */
export type OpenAIResourceWriteOptions = {
  ifMatch?: string;
} & ({ blob: string; text?: never } | { blob?: never; text: string });

/** Parameters for the OpenAI `openai/resources/write` MCP App request. */
export type OpenAIResourceWriteParams = z.infer<
  typeof OpenAIResourceWriteParamsSchema
>;

/** Result of the OpenAI `openai/resources/write` MCP App request. */
export type OpenAIResourceWriteResult = z.infer<
  typeof OpenAIResourceWriteResultSchema
>;

/** Reads a host-managed resource and parses OpenAI-specific resource metadata. */
export type OpenAIResourceRead = (
  params: ReadResourceRequest["params"] & {
    _meta?: OpenAIResourceReadMetadata;
    /** Overrides `_meta["openai/resource"].representation` when both are provided. */
    representation?: OpenAIResourceRepresentation;
  },
  options?: RequestOptions,
) => Promise<OpenAIResourceReadResult>;

/** Handler for a host-managed resource update notification. */
export type OpenAIResourceUpdatedHandler = (
  notification: ResourceUpdatedNotification,
) => void | Promise<void>;

/** Host-managed resource extensions exposed for one MCP App instance. */
export type OpenAIResources = {
  addUpdateHandler(handler: OpenAIResourceUpdatedHandler): () => void;
  read: OpenAIResourceRead;
  subscribe(
    params: SubscribeRequest["params"],
    options?: RequestOptions,
  ): Promise<EmptyResult>;
  unsubscribe(
    params: UnsubscribeRequest["params"],
    options?: RequestOptions,
  ): Promise<EmptyResult>;
  write(
    uri: string,
    content: OpenAIResourceWriteOptions,
    options?: RequestOptions,
  ): Promise<OpenAIResourceWriteResult>;
};

type OpenAIResourceWriteRequest = {
  method: typeof OPENAI_MCP_APP_RESOURCE_WRITE_METHOD;
  params: OpenAIResourceWriteParams;
};

type OpenAIResourceProtocol = {
  request(
    request: OpenAIResourceWriteRequest,
    resultSchema: typeof OpenAIResourceWriteResultSchema,
    options?: RequestOptions,
  ): Promise<OpenAIResourceWriteResult>;
  request(
    request: SubscribeRequest | UnsubscribeRequest,
    resultSchema: typeof EmptyResultSchema,
    options?: RequestOptions,
  ): Promise<EmptyResult>;
  setNotificationHandler(
    notificationSchema: typeof ResourceUpdatedNotificationSchema,
    handler: OpenAIResourceUpdatedHandler,
  ): void;
};

function createResourceRead(app: App): OpenAIResourceRead {
  return async (
    { representation, ...params }: Parameters<OpenAIResourceRead>[0],
    options?: RequestOptions,
  ): Promise<OpenAIResourceReadResult> => {
    const resource = await app.readServerResource(
      representation == null
        ? params
        : {
            ...params,
            _meta: {
              ...params._meta,
              [OPENAI_RESOURCE_METADATA_KEY]: {
                ...params._meta?.[OPENAI_RESOURCE_METADATA_KEY],
                representation,
              },
            },
          },
      options,
    );
    return {
      ...resource,
      contents: resource.contents.map((content) => ({
        ...content,
        openaiMetadata: getOpenAIResourceMetadata(content._meta),
      })),
    };
  };
}

export function createResources(app: App): OpenAIResources {
  const protocol = app as unknown as OpenAIResourceProtocol;
  const updateHandlers = new Set<OpenAIResourceUpdatedHandler>();
  let hasUpdateHandler = false;

  return {
    addUpdateHandler: (handler) => {
      if (!hasUpdateHandler) {
        protocol.setNotificationHandler(
          ResourceUpdatedNotificationSchema,
          async (notification) => {
            await Promise.all(
              Array.from(updateHandlers, async (updateHandler) =>
                updateHandler(notification),
              ),
            );
          },
        );
        hasUpdateHandler = true;
      }

      updateHandlers.add(handler);
      return () => {
        updateHandlers.delete(handler);
      };
    },
    read: createResourceRead(app),
    subscribe: (params, options) =>
      protocol.request(
        { method: "resources/subscribe", params },
        EmptyResultSchema,
        options,
      ),
    unsubscribe: (params, options) =>
      protocol.request(
        { method: "resources/unsubscribe", params },
        EmptyResultSchema,
        options,
      ),
    write: (uri, content, options) =>
      protocol.request(
        {
          method: OPENAI_MCP_APP_RESOURCE_WRITE_METHOD,
          params: OpenAIResourceWriteParamsSchema.parse({ uri, ...content }),
        },
        OpenAIResourceWriteResultSchema,
        options,
      ),
  };
}

function getOpenAIResourceMetadata(
  meta: ReadResourceResult["contents"][number]["_meta"],
): OpenAIResourceMetadata | undefined {
  const parsedMetadata = OpenAIResourceContentMetadataSchema.safeParse(
    meta ?? {},
  );
  if (!parsedMetadata.success) {
    return undefined;
  }

  return parsedMetadata.data[OPENAI_RESOURCE_METADATA_KEY];
}
