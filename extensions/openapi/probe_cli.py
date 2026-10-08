"""Prove the native imported CLI and progressive MCP interface against loopback HTTP.

Build first:
  cargo build --manifest-path extensions/openapi/Cargo.toml --features native-cli --example openapi --locked
Run from any directory:
  python3 extensions/openapi/probe_cli.py
"""
from pathlib import Path
import json
import os
import selectors
import subprocess
import threading
import time
from email.parser import BytesParser
from email import policy
from http.server import BaseHTTPRequestHandler, HTTPServer

def expected_response():
    return {'status': 429, 'headers': [['set-cookie', 'a=1'], ['set-cookie', 'b=2'], ['content-length', '3'], ['connection', 'close']], 'body': [0, 255, 1]}

def prove_cli(binary, root):
    observations = []
    failures = []

    class Handler(BaseHTTPRequestHandler):

        def log_message(self, *args):
            pass

        def do_POST(self):
            try:
                expected = [(b'123', 'text/plain'), (b'', 'text/plain'), (b'', None)][len(observations)]
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                observed = (body, self.headers.get('Content-Type'))
                assert self.path == '/text', self.path
                assert observed == expected, (observed, expected)
                observations.append(observed)
                self.send_response_only(429)
                self.send_header('Set-Cookie', 'a=1')
                self.send_header('Set-Cookie', 'b=2')
                self.send_header('Content-Length', '3')
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(bytes([0, 255, 1]))
            except BaseException as error:
                failures.append(repr(error))
                self.close_connection = True
    server = HTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        uri = f'http://127.0.0.1:{server.server_port}/openapi.json'
        for flags in [['--body', '123'], ['--body-json', '""'], []]:
            r = subprocess.run([binary, str(root / 'extensions/openapi/tests/fixtures/text_openapi.json'), uri, 'op_sendText', *flags, '--format', 'json'], capture_output=True, text=True, timeout=20)
            assert r.returncode == 0, (r.stdout, r.stderr, failures)
            output = json.loads(r.stdout)
            assert output == expected_response(), output
        assert not failures, failures
        assert len(observations) == 3, observations
        return len(observations)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

def check_multipart_request(requests):
    assert len(requests) == 1, requests
    path, body, content_type = requests[0]
    assert path == '/multipart', path
    message = BytesParser(policy=policy.default).parsebytes(('MIME-Version: 1.0\r\nContent-Type: ' + content_type + '\r\n\r\n').encode() + body)
    assert message.is_multipart() and (not message.defects), message.defects
    parts = list(message.iter_parts())
    observed = [(part.get_param('name', header='Content-Disposition'), part.get_content_type(), part.get_payload(decode=True)) for part in parts]
    assert observed == [('address', 'application/json', b'{"city":"New York"}'), ('count', 'text/plain', b'9007199254740993'), ('encoded', 'application/octet-stream', bytes([0, 255, 1])), ('packed', 'text/plain', b'a,b'), ('tags', 'text/plain', b'red blue'), ('tags', 'text/plain', b'+&'), ('text', 'text/plain', '雪 --incurs-openapi-0'.encode())], observed
    assert parts[0]['X-Part'] == 'proof'
    assert parts[2]['Content-Transfer-Encoding'] == 'base64'
    assert message.get_boundary() == 'incurs-openapi-1'
    assert all((not part.defects for part in parts))


class McpClient:
    def __init__(self, argv):
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b''

    def send(self, message):
        self.process.stdin.write(json.dumps(message).encode() + b'\n')
        self.process.stdin.flush()

    def reply(self, identifier):
        deadline = time.monotonic() + 15
        while True:
            if b'\n' in self.buffer:
                line, self.buffer = self.buffer.split(b'\n', 1)
                if not line.strip():
                    continue
                value = json.loads(line)
                if value.get('id') == identifier:
                    return value
                assert 'id' not in value, value
                continue
            remaining = deadline - time.monotonic()
            assert remaining > 0 and self.selector.select(remaining), 'MCP reply timed out'
            chunk = os.read(self.process.stdout.fileno(), 65536)
            assert chunk, 'MCP closed stdout'
            self.buffer += chunk

    def close(self):
        try:
            self.process.stdin.close()
            try:
                code = self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                raise
            assert code == 0, self.process.stderr.read().decode()
        finally:
            self.selector.close()
            self.process.stdout.close()
            self.process.stderr.close()

def prove_mcp(binary, root, form=False, multipart=False):
    name = 'sendMultipart' if multipart else 'sendForm' if form else 'sendText'
    fixture = 'multipart_openapi.json' if multipart else 'form_openapi.json' if form else 'text_openapi.json'
    body = {'text': 'a+b &雪', 'count': 9007199254740993, 'address': {'city': 'New York'}, 'quoted': '123', 'packed': ['a,b', 'c d'], 'tags': ['red blue', '+&']} if form else 'mcp-雪'
    wire = b'address=%7B%22city%22%3A%22New+York%22%7D&count=9007199254740993&packed=a%2Cb,c%20d&quoted=%22123%22&tags=red%20blue&tags=%2B%26&text=a%2Bb+%26%E9%9B%AA' if form else 'mcp-雪'.encode()
    if multipart:
        body = {'text': '雪 --incurs-openapi-0', 'count': 9007199254740993, 'encoded': 'AP8B', 'tags': ['red blue', '+&'], 'packed': ['a', 'b'], 'address': {'city': 'New York'}}
    requests = []

    class Handler(BaseHTTPRequestHandler):

        def log_message(self, *args):
            pass

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            requests.append((self.path, body, self.headers.get('Content-Type')))
            self.send_response_only(429)
            self.send_header('Set-Cookie', 'a=1')
            self.send_header('Set-Cookie', 'b=2')
            self.send_header('Content-Length', '3')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(bytes([0, 255, 1]))
    server = HTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    client = McpClient([binary, str(root / 'extensions/openapi/tests/fixtures' / fixture), f'http://127.0.0.1:{server.server_port}/openapi.json', '--mcp'])
    send, reply = client.send, client.reply
    try:
        send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-06-18', 'capabilities': {}, 'clientInfo': {'name': 'openapi-proof', 'version': '1'}}})
        initialized = reply(1)
        assert initialized['result']['protocolVersion'] == '2025-06-18', initialized
        send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        send({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list', 'params': {}})
        listed = reply(2)
        tools = listed['result']['tools']
        assert {tool['name'] for tool in tools} == {'search_tools', 'get_tool_details', 'call_read_tool', 'call_write_tool'}, tools
        send({'jsonrpc': '2.0', 'id': 20, 'method': 'tools/call', 'params': {'name': 'search_tools', 'arguments': {'query': name}}})
        searched = reply(20)
        assert [tool['name'] for tool in searched['result']['structuredContent']['tools']] == ['op_' + name], searched
        send({'jsonrpc': '2.0', 'id': 21, 'method': 'tools/call', 'params': {'name': 'get_tool_details', 'arguments': {'name': 'op_' + name}}})
        detailed = reply(21)
        assert detailed['result']['structuredContent']['inputSchema']['properties']['body']['type'] == ('object' if form or multipart else 'string'), detailed
        send({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {'name': 'call_write_tool', 'arguments': {'name': 'op_' + name, 'arguments': {'body': body}}}})
        called = reply(3)
        result = called['result']
        assert result.get('isError') is not True, result
        expected = {'status': 429, 'headers': [['set-cookie', 'a=1'], ['set-cookie', 'b=2'], ['content-length', '3'], ['connection', 'close']], 'body': [0, 255, 1]}
        assert result['structuredContent'] == expected, result
        assert json.loads(result['content'][0]['text']) == expected, result
        if multipart:
            check_multipart_request(requests)
        else:
            assert requests == [('/form' if form else '/text', wire, 'application/x-www-form-urlencoded' if form else 'text/plain')], requests
        send({'jsonrpc': '2.0', 'id': 4, 'method': 'tools/call', 'params': {'name': 'call_write_tool', 'arguments': {'name': 'op_' + name, 'arguments': {'body': []}}}})
        rejected = reply(4)
        assert rejected['result']['isError'] is True, rejected
        error = json.loads(rejected['result']['content'][0]['text'])
        assert error == {'code': 'OPENAPI_BINDING', 'exit_code': 1, 'message': 'multipart body must be an object' if multipart else 'URL-encoded body must be an object' if form else 'text/plain body must be a string'}, error
        assert rejected['result']['structuredContent'] == error, rejected
        assert len(requests) == 1, requests
        return {'requests': len(requests), 'protocol_version': initialized['result']['protocolVersion']}
    finally:
        try:
            client.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def prove_openapi32(binary, root):
    observations = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def observe(self):
            body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            observations.append((self.command, self.path, self.headers.get('Cookie'), self.headers.get('X-Meta'), body))
            self.send_response_only(204)
            self.send_header('Content-Length', '0')
            self.send_header('Connection', 'close')
            self.end_headers()
        do_QUERY = observe
        do_GET = observe
    setattr(Handler, 'do_x-Copy', Handler.observe)
    server = HTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    fixture = str(root / 'extensions/openapi/tests/fixtures/openapi32.json')
    assert json.loads(Path(fixture).read_text())['servers'] == [{'url': '/'}]
    uri = f'http://127.0.0.1:{server.server_port}/openapi.json'
    cases = [
        ('findItems', {'querystring': {'filter': {'term': 'a + b/é', 'labels': ['red', 'blue']}}, 'cookie': {'prefs': {'mode': 'dark', 'token': 'a%2Fb'}}},
         ('QUERY', '/find?labels=red&labels=blue&term=a+%2B+b%2F%C3%A9', 'mode=dark; token=a%2Fb', None, b'')),
        ('copyItem', {'path': {'id': 'a/b'}, 'header': {'X-Meta': {'id': 9007199254740993}}},
         ('x-Copy', '/copy/a%2Fb', None, '{"id":9007199254740993}', b'')),
        ('readRaw', {}, ('GET', '/raw', None, None, b'')),
        ('readRaw', {'querystring': {'raw': ''}}, ('GET', '/raw?', None, None, b'')),
        ('readRaw', {'querystring': {'raw': 'x=a%2Fb&x=c+d'}}, ('GET', '/raw?x=a%2Fb&x=c+d', None, None, b'')),
    ]
    client = None
    try:
        for name, arguments, expected in cases:
            flags = [item for key, value in arguments.items() for item in ('--' + key, json.dumps(value))]
            run = subprocess.run([binary, fixture, uri, 'op_' + name, *flags, '--format', 'json'], capture_output=True, text=True, timeout=20)
            assert run.returncode == 0, (run.stdout, run.stderr)
            assert json.loads(run.stdout)['status'] == 204, run.stdout
            assert observations[-1] == expected, observations
        client = McpClient([binary, fixture, uri, '--mcp'])
        client.send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-06-18', 'capabilities': {}, 'clientInfo': {'name': 'openapi-32-proof', 'version': '1'}}})
        assert client.reply(1)['result']['protocolVersion'] == '2025-06-18'
        client.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        client.send({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {'name': 'get_tool_details', 'arguments': {'name': 'op_findItems'}}})
        schema = client.reply(2)['result']['structuredContent']['inputSchema']
        assert schema['properties']['querystring']['properties']['filter']['type'] == 'object', schema
        assert 'querystring' in schema['required'], schema
        assert schema['properties']['querystring']['required'] == ['filter'], schema
        for identifier, (name, arguments, expected) in enumerate(cases, 3):
            client.send({'jsonrpc': '2.0', 'id': identifier, 'method': 'tools/call', 'params': {'name': 'call_write_tool', 'arguments': {'name': 'op_' + name, 'arguments': arguments}}})
            reply = client.reply(identifier)['result']
            assert reply.get('isError') is not True and reply['structuredContent']['status'] == 204, reply
            assert observations[-1] == expected, observations
        client.send({'jsonrpc': '2.0', 'id': 50, 'method': 'tools/call', 'params': {'name': 'call_write_tool', 'arguments': {'name': 'op_readRaw', 'arguments': {'querystring': {'raw': 'x=a#fragment'}}}}})
        rejected = client.reply(50)['result']
        assert rejected['isError'] is True, rejected
        assert rejected['structuredContent']['code'] == 'OPENAPI_BINDING', rejected
        assert observations == [case[2] for case in cases] * 2, observations
        return len(observations)
    finally:
        try:
            if client:
                client.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

def media_cases():
    return [
        ("uploadBinary", {}, ("/binary", None, b"")),
        ("uploadBinary", {"body_base64": ""}, ("/binary", "application/octet-stream", b"")),
        ("uploadBinary", {"body_base64": "AP+ADQo="}, ("/binary", "application/octet-stream", bytes([0,255,128,13,10]))),
        ("sendMarkdown", {"body":"snow 雪\n","media_type":"text/plain"}, ("/markdown", "text/plain", "snow 雪\n".encode())),
        ("sendMarkdown", {"body":"snow 雪\n","media_type":"text/x-markdown"}, ("/markdown", "text/x-markdown", "snow 雪\n".encode())),
        ("sendScript", {"body":"export default 1;","media_type":"application/javascript"}, ("/scripts", "application/javascript", b"export default 1;")),
        ("uploadRecords", {"body":"{\"id\":1}\nnull"}, ("/records", "application/jsonl", b"{\"id\":1}\nnull")),
        ("uploadNdjson", {"body_base64":"eyJpZCI6MX0K"}, ("/ndjson", "application/x-ndjson", b"{\"id\":1}\n")),
        ("sendMixed", {"body":None,"media_type":"application/json"}, ("/mixed", "application/json", b"null")),
        ("sendMixed", {"body_base64":"AP+ADQo=","media_type":"application/octet-stream"}, ("/mixed", "application/octet-stream", bytes([0,255,128,13,10]))),
    ]

def invalid_media_cases():
    return [
        ("uploadBinary", {"body":"raw"}),
        ("uploadBinary", {"body_base64":"!"}),
        ("uploadBinary", {"body_base64":None}),
        ("uploadBinary", {"body":"raw","body_base64":"cmF3"}),
        ("uploadRecords", {"body":"1\n\n"}),
        ("uploadRecords", {"body":[1,2]}),
        ("uploadNdjson", {"body":"1\n"}),
        ("uploadNdjson", {"body_base64":"MQ=="}),
        ("sendMixed", {"body":None,"media_type":"application/x-forbidden+json"}),
        ("sendMixed", {"body_base64":""}),
        ("sendMixed", {"body_base64":"","media_type":"application/json"}),
        ("sendConfig", {"body":{"name":"x"},"media_type":"text/plain;charset=UTF-8"}),
    ]

def prove_media(binary, root):
    import tempfile
    observations = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length",0)))
            observations.append((self.path,self.headers.get("Content-Type"),body))
            self.send_response_only(204)
            self.send_header("Content-Length","0")
            self.send_header("Connection","close")
            self.end_headers()
    server=HTTPServer(("127.0.0.1",0),Handler)
    thread=threading.Thread(target=server.serve_forever,daemon=True)
    thread.start()
    client=None
    try:
        with tempfile.TemporaryDirectory(prefix="openapi-media-probe-") as directory:
            fixture=Path(directory)/"openapi.json"
            document=json.loads((root/"extensions/openapi/tests/fixtures/media_openapi.json").read_text())
            document["servers"]=[{"url":"/"}]
            fixture.write_text(json.dumps(document))
            uri=f"http://127.0.0.1:{server.server_port}/openapi.json"
            def cli_flags(arguments):
                flags=[]
                for key,value in arguments.items():
                    if key=="body":
                        flags.extend(["--body-json",json.dumps(value)])
                    else:
                        flags.extend(["--"+key.replace("_","-"),value if isinstance(value,str) else json.dumps(value)])
                return flags
            for name,arguments,expected in media_cases():
                result=subprocess.run([binary,str(fixture),uri,"op_"+name,*cli_flags(arguments),"--format","json"],capture_output=True,text=True,timeout=20)
                assert result.returncode==0,(result.stdout,result.stderr)
                assert json.loads(result.stdout)["status"]==204,result.stdout
                assert observations[-1]==expected,(observations[-1],expected)
            cli_rejections = 0
            for name,arguments in invalid_media_cases():
                # A CLI string flag cannot express a typed null; MCP covers that case.
                if arguments.get("body_base64", "") is None:
                    continue
                cli_rejections += 1
                count=len(observations)
                result=subprocess.run([binary,str(fixture),uri,"op_"+name,*cli_flags(arguments),"--format","json"],capture_output=True,text=True,timeout=20)
                assert result.returncode!=0,(name,result.stdout,result.stderr)
                assert len(observations)==count,observations
            client=McpClient([binary,str(fixture),uri,"--mcp"])
            client.send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"openapi-media-proof","version":"1"}}})
            assert client.reply(1)["result"]["protocolVersion"]=="2025-06-18"
            client.send({"jsonrpc":"2.0","method":"notifications/initialized"})
            client.send({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_tool_details","arguments":{"name":"op_uploadBinary"}}})
            schema=client.reply(2)["result"]["structuredContent"]["inputSchema"]
            assert schema["properties"]["body_base64"]=={"type":"string","contentEncoding":"base64"},schema
            assert "body" not in schema["properties"] and "body_json" not in schema["properties"],schema
            for identifier,(name,arguments,expected) in enumerate(media_cases(),10):
                client.send({"jsonrpc":"2.0","id":identifier,"method":"tools/call","params":{"name":"call_write_tool","arguments":{"name":"op_"+name,"arguments":arguments}}})
                result=client.reply(identifier)["result"]
                assert result.get("isError") is not True,result
                assert result["structuredContent"]["status"]==204,result
                assert observations[-1]==expected,(observations[-1],expected)
            for identifier,(name,arguments) in enumerate(invalid_media_cases(),100):
                count=len(observations)
                client.send({"jsonrpc":"2.0","id":identifier,"method":"tools/call","params":{"name":"call_write_tool","arguments":{"name":"op_"+name,"arguments":arguments}}})
                result=client.reply(identifier)["result"]
                assert result["isError"] is True,result
                assert result["structuredContent"]["code"] in ("OPENAPI_BINDING","OPENAPI_ARGUMENTS","VALIDATION_ERROR"),result
                assert len(observations)==count,observations
            assert observations==[expected for _,_,expected in media_cases()]*2
            return {"requests":len(observations),"cli_rejections":cli_rejections,"mcp_rejections":len(invalid_media_cases())}
    finally:
        try:
            if client:
                client.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

def main():
    root = Path(__file__).resolve().parents[2]
    binary = str(root / 'extensions/openapi/target/debug/examples/openapi')
    if not Path(binary).is_file():
        raise SystemExit('Build the openapi example with --features native-cli first.')
    media = prove_media(binary, root)
    cli_requests = prove_cli(binary, root)
    mcp = prove_mcp(binary, root)
    form_mcp = prove_mcp(binary, root, form=True)
    multipart_mcp = prove_mcp(binary, root, multipart=True)
    openapi32_requests = prove_openapi32(binary, root)
    print(json.dumps({'passed': True, 'media': media, 'openapi32_requests': openapi32_requests, 'cli_requests': cli_requests, 'mcp_requests': mcp['requests'], 'form_mcp_requests': form_mcp['requests'], 'multipart_mcp_requests': multipart_mcp['requests'], 'mcp_protocol_version': mcp['protocol_version'], 'verified': ['raw-string-body', 'empty-body', 'omitted-body', 'binary-http-error-response', 'duplicate-response-headers', 'mcp-initialize', 'mcp-search', 'mcp-inspect', 'mcp-call', 'mcp-binding-error-without-http', 'form-mcp-wire-encoding', 'form-mcp-error-without-http', 'multipart-independent-MIME-parser', 'multipart-boundary-collision', 'multipart-referenced-part-header', 'multipart-base64-transfer', 'multipart-error-without-http', 'openapi32-QUERY', 'custom-method-case', 'querystring-content', 'cookie-style', 'parameter-content', 'querystring-rejection-without-http'], 'scope': 'Bounded loopback fixtures; not universal OpenAPI conformance.'}, indent=2))
if __name__ == '__main__':
    main()
