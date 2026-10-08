"""Verify current generated SDK validation and media binding through Worker HTTP."""
import base64
import concurrent.futures
import json
import sys
import urllib.request

VALIDATION = {
    "body": "4",
    "recursive": '{"next":{"value":2},"value":1}',
    "overlap": "3",
    "exact": "18446744073709551615",
    "invalid_low": True,
    "invalid_pattern": True,
    "invalid_multiple": True,
    "impossible": True,
    "decimal": True,
    "invalid_decimal": True,
}
CASES = [
    ("/binary", None, None, False),
    ("/binary", "application/octet-stream", b"", True),
    ("/binary", "application/octet-stream", bytes([0, 255, 128, 13, 10]), True),
    ("/binary", "application/octet-stream", bytes(range(256)), True),
    ("/binary", "application/octet-stream", bytes(range(255)), True),
    ("/binary", "application/octet-stream", bytes(range(254)), True),
    ("/markdown", "text/plain", "snow 雪\n".encode(), False),
    ("/markdown", "text/x-markdown", "snow 雪\n".encode(), False),
    ("/records", "application/jsonl", b'{"id":1}\nnull', False),
    ("/ndjson", "application/x-ndjson", b'{"id":1}\n', True),
    ("/mixed", "application/json", b"null", False),
    ("/mixed", "application/json", b"null", False),
]


def verify(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        assert response.status == 200, response.status
        result = json.load(response)
    assert result["validation"] == VALIDATION, result["validation"]
    assert result["response_statuses"] == {
        "classifications": 196608, "layouts": 3, "raw_body_and_duplicate_headers": True,
    }, result["response_statuses"]
    assert result["typed_responses"] == {
        "id": "18446744073709551617",
        "amount": "0.12345678901234567890123456789",
        "stamp": "server",
        "labels": [None, "hi"],
        "nested_integer": "4",
        "write_only_omitted": True,
        "default_not_inserted": True,
        "extras": '{"extra":{"kept":true}}',
        "raw_body_retained": True,
        "cookies": ["a=1", "b=2"],
        "media_type": "application/json",
        "error_kinds": ["Json", "MediaType", "Schema"],
    }, result["typed_responses"]
    requests = result["request_validation"]
    assert requests["schema_rejections"] == 18, requests
    expected_request_bodies = [
        '{"profile":{"age":21,"child":{"age":22,"state":"active"},"id":9007199254740993,"mode":5,"name":"Alice","roles":["admin"],"state":"active"}}',
        '{"other":true}',
    ]
    assert requests["requests"] == [
        {"url": "https://example.test/submit?limit=3", "body": expected_request_bodies[0]},
        {"url": "https://example.test/submit", "body": expected_request_bodies[1]},
    ], requests
    numeric = result["numeric"]
    assert numeric["valid_bound"] is True, numeric
    assert numeric["invalid_bound"] is True, numeric
    assert numeric["malformed_rejected"] is True, numeric
    assert numeric["url"] == (
        "https://example.test/numbers?high=18446744073709551617&low=-9223372036854775809"
    ), numeric
    expected_body = ('{"huge":1e+400,"nested":[{"high":18446744073709551617,'
                     '"precise":0.100000000000000000000000000001}],'
                     '"precise":0.12345678901234567890123456789,"tiny":1e-400}')
    assert numeric["body"] == expected_body, numeric
    assert '"high":18446744073709551617' in numeric["arguments"], numeric
    assert '"low":-9223372036854775809' in numeric["arguments"], numeric
    assert expected_body in numeric["arguments"], numeric
    media = result["media"]
    assert media["false_body"] is True, media
    assert media["false_default"] is True, media
    assert media["conflicting_slots"] is True, media
    assert len(media["requests"]) == len(CASES), media
    for observed, (path, content_type, body, binary) in zip(media["requests"], CASES):
        assert observed["url"] == "https://example.test" + path, observed
        assert observed["method"] == "POST", observed
        assert observed["content_type"] == content_type, observed
        assert observed["body"] == (list(body) if body is not None else None), observed
        arguments = observed["arguments"]
        if binary:
            assert arguments["body_base64"] == base64.b64encode(body).decode("ascii"), observed
            assert "body" not in arguments and "body_json" not in arguments, observed
        elif body is None:
            assert all(key not in arguments for key in ("body", "body_json", "body_base64")), observed
        else:
            assert "body_base64" not in arguments, observed
    return result


def main():
    url = sys.argv[1] if len(sys.argv) == 2 else "http://127.0.0.1:18988"
    initial = verify(url)
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(verify, [url] * 8))
    assert all(result == initial for result in results)
    print(json.dumps({
        "passed": True,
        "generated_media_bindings": len(CASES),
        "response_statuses": initial["response_statuses"],
        "typed_responses": initial["typed_responses"],
        "sdk_rejections": 3,
        "request_schema_rejections": 18,
        "validated_request_cases": 2,
        "numeric_defaults": "exact query integers, decimal/exponent and nested body defaults",
        "numeric_controls": ["exact bound accepted", "adjacent integer rejected", "malformed token rejected"],
        "base64_byte_values": 256,
        "base64_nonempty_padding_remainders": [0, 1, 2],
        "concurrent_requests": 8,
        "validation": initial["validation"],
        "scope": "Current SDK package archives -> Emscripten -> request binding, observed through local HTTP; no vendor request."
    }, indent=2))


if __name__ == "__main__":
    main()
