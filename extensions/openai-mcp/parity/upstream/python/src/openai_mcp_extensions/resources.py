"""Host-owned file entrypoint and resource metadata."""

from __future__ import annotations

from pydantic import Field

from openai_mcp_extensions._models import NonBlankString, OpenAIModel

OPENAI_RESOURCE_METADATA_KEY = "openai/resource"


class OpenAIFileEntrypointFile(OpenAIModel):
    """A selected file supplied by an OpenAI file entrypoint."""

    name: NonBlankString
    resource_uri: NonBlankString


class OpenAIFileEntrypointInput(OpenAIModel):
    """Arguments supplied when an MCP App opens through a file entrypoint."""

    file: OpenAIFileEntrypointFile


class OpenAIResourceMetadata(OpenAIModel):
    """Host-owned resource context attached to a tool call."""

    path: str


class OpenAIResourceToolCallMetadata(OpenAIModel):
    """Forward-compatible reader for OpenAI resource tool-call metadata."""

    resource: OpenAIResourceMetadata | None = Field(
        default=None,
        alias=OPENAI_RESOURCE_METADATA_KEY,
    )


def get_resource_path(meta: object = None) -> str | None:
    """Read the host-owned file path from tool-call metadata, validating its shape."""
    metadata = OpenAIResourceToolCallMetadata.model_validate(
        {} if meta is None else meta, by_alias=True, by_name=False
    )
    return metadata.resource.path if metadata.resource is not None else None
