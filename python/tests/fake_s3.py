"""A fake S3 server for the tests: checks every signature with an
independent SigV4 written in Python (hmac/hashlib), keeps objects in
memory, pages listings by two and misbehaves on demand."""

from __future__ import annotations

import hashlib
import hmac
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qsl, unquote, urlsplit

ACCESS = "GKtest"
SECRET = "secret-of-the-fake"
REGION = "us-east-1"
PAGE = 2

_UNRESERVED = set(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~")


def enc(s: str) -> str:
    return "".join(chr(b) if b in _UNRESERVED else f"%{b:02X}" for b in s.encode("utf-8"))


def py_sign(method, host, path, query, payload_hash, amz_date, access, secret, region):
    pairs = sorted((enc(k), enc(v)) for k, v in query)
    cq = "&".join(f"{k}={v}" for k, v in pairs)
    headers = sorted([("host", host), ("x-amz-content-sha256", payload_hash), ("x-amz-date", amz_date)])
    ch = "".join(f"{k}:{v}\n" for k, v in headers)
    sh = ";".join(k for k, _ in headers)
    creq = f"{method}\n{path}\n{cq}\n{ch}\n{sh}\n{payload_hash}"
    date = amz_date[:8]
    scope = f"{date}/{region}/s3/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(creq.encode()).hexdigest()}"

    def h(k, m):
        return hmac.new(k, m.encode(), hashlib.sha256).digest()

    k = h(h(h(h(("AWS4" + secret).encode(), date), region), "s3"), "aws4_request")
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    return f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={sh}, Signature={sig}"


class Store:
    def __init__(self):
        self.objects: dict[tuple[str, str], tuple[bytes, str]] = {}
        self.requests: list[str] = []
        self.fail_first = 0
        self.lock = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    store: Store

    def log_message(self, *a):  # silence
        pass

    def _reply(self, status, headers=(), body=b""):
        self.send_response(status)
        self.send_header("Connection", "close")
        if "content-length" not in {k.lower() for k, _ in headers}:
            self.send_header("Content-Length", str(len(body)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _handle(self):
        store = self.store
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        split = urlsplit(self.path)
        raw_path, query = split.path, parse_qsl(split.query, keep_blank_values=True)
        host = self.headers["Host"]
        payload_hash = self.headers.get("x-amz-content-sha256", "")
        assert payload_hash == hashlib.sha256(body).hexdigest(), "payload hash"
        expected = py_sign(self.command, host, raw_path, query, payload_hash, self.headers["x-amz-date"], ACCESS, SECRET, REGION)
        ok = self.headers.get("Authorization") == expected
        segs = unquote(raw_path).lstrip("/").split("/", 1)
        bucket = segs[0]
        key = segs[1] if len(segs) > 1 else ""
        with store.lock:
            store.requests.append(f"{self.command} {self.path}")
            if not ok:
                return self._reply(403, body=b"<Error><Code>SignatureDoesNotMatch</Code><Message>bad signature</Message></Error>")
            if bucket == "forbidden":
                return self._reply(403, body=b"<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>")
            if "flaky" in key and store.fail_first > 0:
                store.fail_first -= 1
                return self._reply(503, body=b"<Error><Code>SlowDown</Code><Message>try later</Message></Error>")
            if bucket == "":
                return self._reply(200, [("Content-Type", "application/xml")],
                                   b"<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>")
            if self.command == "GET" and key == "":
                q = dict(query)
                prefix = q.get("prefix", "")
                start = int(q.get("continuation-token", 0))
                keys = sorted(k for (b, k) in store.objects if b == bucket and k.startswith(prefix))
                page = keys[start:start + PAGE]
                truncated = start + PAGE < len(keys)
                xml = '<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">'
                for k in page:
                    data, _ = store.objects[(bucket, k)]
                    kx = k.replace("&", "&amp;").replace("<", "&lt;")
                    xml += f'<Contents><Key>{kx}</Key><Size>{len(data)}</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>"{hashlib.md5(data).hexdigest()}"</ETag></Contents>'
                xml += f"<IsTruncated>{'true' if truncated else 'false'}</IsTruncated>"
                if truncated:
                    xml += f"<NextContinuationToken>{start + PAGE}</NextContinuationToken>"
                xml += "</ListBucketResult>"
                return self._reply(200, [("Content-Type", "application/xml")], xml.encode())
            if self.command == "PUT":
                etag = "0" * 32 if key == "bad-etag" else hashlib.md5(body).hexdigest()
                store.objects[(bucket, key)] = (body, self.headers.get("Content-Type", ""))
                return self._reply(200, [("ETag", f'"{etag}"')])
            if self.command == "DELETE":
                store.objects.pop((bucket, key), None)
                return self._reply(204)
            if (bucket, key) in store.objects:
                data, ct = store.objects[(bucket, key)]
                headers = [("ETag", f'"{hashlib.md5(data).hexdigest()}"'), ("Content-Type", ct),
                           ("Last-Modified", "Thu, 23 Oct 2026 12:00:00 GMT"), ("Content-Length", str(len(data)))]
                return self._reply(200, headers, data)
            return self._reply(404, body=b"<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>")

    do_GET = do_PUT = do_HEAD = do_DELETE = _handle


def start():
    store = Store()
    handler = type("H", (Handler,), {"store": store})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, store, f"http://127.0.0.1:{server.server_port}"
