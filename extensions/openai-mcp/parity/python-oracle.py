#!/usr/bin/env python3
"""JSONL observations from the independently pinned Python form SDK."""
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent / "upstream/python/src"))
from openai_mcp_form_protocol import (FormField, FormSchema, is_valid_value,
    prepare_field_submission, complete_field_submission,
    validate_file_selections, validate_form_selections)

def observe(request):
    operation = request["operation"]
    if operation.startswith("validate_"):
        form = FormSchema[FormField].model_validate(request["form"])
        callback = {"validate_file_selections": validate_file_selections,
                    "validate_form_selections": validate_form_selections}[operation]
        callback(form, request["content"])
        return None
    field = FormField.model_validate(request["field"])
    if operation == "is_valid_value":
        return is_valid_value(field, request["value"],
            pending_uploads=request.get("pending_uploads", 0),
            uploaded_uris=tuple(request.get("uploaded_uris", [])))
    if operation == "prepare_field_submission":
        return prepare_field_submission(field, request["name"], request["content"],
            pending_uploads=request.get("pending_uploads", 0))
    if operation == "complete_field_submission":
        return complete_field_submission(field, request["name"], request["content"],
            tuple(request["uploaded_uris"]))
    raise ValueError("Unknown operation")

for line in sys.stdin:
    try:
        print(json.dumps({"ok": True, "value": observe(json.loads(line))}))
    except Exception:
        print(json.dumps({"ok": False, "error": "validation_error"}))
