"""Native MCP Python SDK integration for OpenAI server extensions."""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Literal

from mcp.server.elicitation import ElicitationResult, ElicitSchemaModelT
from mcp.server.extension import Extension, ToolBinding
from mcp.server.mcpserver.context import Context

from openai_mcp_extensions.form import elicit_form
from openai_mcp_extensions.mentions import OpenAIMentions


class OpenAIExtensions(Extension):
    """Compose OpenAI-specific server behavior into an MCP Python server."""

    identifier = "openai/extensions"

    def __init__(self) -> None:
        self.mentions = OpenAIMentions()

    def tools(self) -> Sequence[ToolBinding]:
        """Contribute only the OpenAI tools explicitly configured by the server."""

        return self.mentions.tools()

    async def elicit_input(
        self,
        context: Context[Any, Any],
        *,
        mode: Literal["form"],
        message: str,
        schema: type[ElicitSchemaModelT],
    ) -> ElicitationResult[ElicitSchemaModelT]:
        """Request input and return the validated Pydantic model."""

        if mode != "form":
            raise ValueError("Unsupported OpenAI elicitation mode")
        return await elicit_form(context, message=message, schema=schema)
