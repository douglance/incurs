"""Shared Pydantic models and string validation."""

from __future__ import annotations

from typing import Annotated

from pydantic import BaseModel, ConfigDict, StringConstraints
from pydantic.alias_generators import to_camel

NonBlankString = Annotated[str, StringConstraints(pattern=r"\S")]


class OpenAIModel(BaseModel):
    """Base model with Python field names and camelCase JSON aliases."""

    model_config = ConfigDict(
        alias_generator=to_camel,
        validate_by_name=True,
    )


class OpenAIStrictModel(OpenAIModel):
    """Base model that rejects unknown fields."""

    model_config = ConfigDict(extra="forbid")
