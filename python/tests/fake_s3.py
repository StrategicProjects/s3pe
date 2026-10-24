"""A fake S3 server for the tests: checks every signature with an
independent SigV4 written in Python (header and query styles), keeps
objects, metadata and multipart uploads in memory, pages listings by two,
honours delimiters, copies, batch-deletes and misbehaves on demand."""

from __future__ import annotations

import hashlib
import hmac
import base64
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


def _signing_key(secret, date, region):
    def h(k, m):
        return hmac.new(k, m.encode(), hashlib.sha256).digest()
    return h(h(h(h(("AWS4" + secret).encode(), date), region), "s3"), "aws4_request")


def _canonical_query(query):
    return "&".join(f"{k}={v}" for k, v in sorted((enc(k), enc(v)) for k, v in query))


def py_sign(method, host, path, query, payload_hash, amz_date, access, secret, region, extra=()):
    headers = sorted([("host", host), ("x-amz-content-sha256", payload_hash), ("x-amz-date", amz_date)] + list(extra))
    ch = "".join(f"{k}:{v}\n" for k, v in headers)
    sh = ";".join(k for k, _ in headers)
    creq = f"{method}\n{path}\n{_canonical_query(query)}\n{ch}\n{sh}\n{payload_hash}"
    date = amz_date[:8]
    scope = f"{date}/{region}/s3/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(creq.encode()).hexdigest()}"
    sig = hmac.new(_signing_key(secret, date, region), sts.encode(), hashlib.sha256).hexdigest()
    return f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={sh}, Signature={sig}"


def py_presign(method, host, path, query, amz_date, expires, access, secret, region):
    date = amz_date[:8]
    scope = f"{date}/{region}/s3/aws4_request"
    params = list(query) + [("X-Amz-Algorithm", "AWS4-HMAC-SHA256"), ("X-Amz-Credential", f"{access}/{scope}"),
                            ("X-Amz-Date", amz_date), ("X-Amz-Expires", str(expires)), ("X-Amz-SignedHeaders", "host")]
    cq = _canonical_query(params)
    creq = f"{method}\n{path}\n{cq}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD"
    sts = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(creq.encode()).hexdigest()}"
    sig = hmac.new(_signing_key(secret, date, region), sts.encode(), hashlib.sha256).hexdigest()
    return f"{cq}&X-Amz-Signature={sig}"


def multipart_etag(md5s_hex):
    return hashlib.md5(b"".join(bytes.fromhex(h) for h in md5s_hex)).hexdigest() + f"-{len(md5s_hex)}"


class Store:
    def __init__(self):
        self.objects: dict[tuple[str, str], dict] = {}   # {"data", "ct", "meta", "etag"}
        self.uploads: dict[str, dict] = {}                  # id -> {"parts": {n: bytes}, "ct", "meta"}
        self.requests: list[str] = []
        self.fail_first = 0
        self.fail_part = None
        self.n_uploads = 0
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

    def _error(self, status, code, msg):
        return self._reply(status, body=f"<Error><Code>{code}</Code><Message>{msg}</Message></Error>".encode())

    def _handle(self):
        store = self.store
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        split = urlsplit(self.path)
        raw_path, query = split.path, parse_qsl(split.query, keep_blank_values=True)
        q = dict(query)
        host = self.headers["Host"]
        if "X-Amz-Signature" in q:
            plain = [(k, v) for k, v in query if not k.startswith("X-Amz-")]
            expected = py_presign(self.command, host, raw_path, plain, q["X-Amz-Date"], int(q["X-Amz-Expires"]), ACCESS, SECRET, REGION)
            ok = expected.endswith("X-Amz-Signature=" + q["X-Amz-Signature"])
        else:
            payload_hash = self.headers.get("x-amz-content-sha256", "")
            assert payload_hash == hashlib.sha256(body).hexdigest(), "payload hash"
            extra = [(k.lower(), v.strip()) for k, v in self.headers.items()
                     if k.lower().startswith("x-amz-") and k.lower() not in ("x-amz-date", "x-amz-content-sha256")]
            expected = py_sign(self.command, host, raw_path, query, payload_hash, self.headers["x-amz-date"], ACCESS, SECRET, REGION, extra)
            ok = self.headers.get("Authorization") == expected
        segs = unquote(raw_path).lstrip("/").split("/", 1)
        bucket = segs[0]
        key = segs[1] if len(segs) > 1 else ""
        meta_in = {k.lower()[11:]: v for k, v in self.headers.items() if k.lower().startswith("x-amz-meta-")}
        with store.lock:
            store.requests.append(f"{self.command} {self.path}")
            if not ok:
                return self._error(403, "SignatureDoesNotMatch", "bad signature")
            if bucket == "forbidden":
                return self._error(403, "AccessDenied", "Access Denied")
            if "flaky" in key and store.fail_first > 0:
                store.fail_first -= 1
                return self._error(503, "SlowDown", "try later")
            if bucket == "":
                return self._reply(200, [("Content-Type", "application/xml")],
                                   b"<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>")
            if self.command == "POST" and key == "" and "delete" in q:
                assert self.headers.get("Content-MD5") == base64.b64encode(hashlib.md5(body).digest()).decode(), "Content-MD5"
                text = body.decode()
                out = "<DeleteResult>"
                for part in text.split("<Key>")[1:]:
                    k = part.split("</Key>")[0].replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
                    if k == "undeletable":
                        out += "<Error><Key>undeletable</Key><Code>AccessDenied</Code><Message>nope</Message></Error>"
                    else:
                        store.objects.pop((bucket, k), None)
                out += "</DeleteResult>"
                return self._reply(200, [("Content-Type", "application/xml")], out.encode())
            if self.command == "GET" and key == "":
                prefix = q.get("prefix", "")
                delim = q.get("delimiter")
                start = int(q.get("continuation-token", 0))
                keys, prefixes = [], []
                for (b, k) in sorted(store.objects):
                    if b != bucket or not k.startswith(prefix) or k.startswith("__init__/"):
                        continue
                    rest = k[len(prefix):]
                    if delim and delim in rest:
                        cp = prefix + rest.split(delim)[0] + delim
                        if cp not in prefixes:
                            prefixes.append(cp)
                    else:
                        keys.append(k)
                page = keys[start:start + PAGE]
                truncated = start + PAGE < len(keys)
                xml = '<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">'
                for k in page:
                    o = store.objects[(bucket, k)]
                    kx = k.replace("&", "&amp;").replace("<", "&lt;")
                    xml += f'<Contents><Key>{kx}</Key><Size>{len(o["data"])}</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>"{o["etag"]}"</ETag></Contents>'
                if start == 0:
                    for p in prefixes:
                        xml += f"<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>"
                xml += f"<IsTruncated>{'true' if truncated else 'false'}</IsTruncated>"
                if truncated:
                    xml += f"<NextContinuationToken>{start + PAGE}</NextContinuationToken>"
                xml += "</ListBucketResult>"
                return self._reply(200, [("Content-Type", "application/xml")], xml.encode())
            if self.command == "POST" and "uploads" in q:
                store.n_uploads += 1
                uid = f"upload-{store.n_uploads}"
                store.uploads[uid] = {"parts": {}, "ct": self.headers.get("Content-Type", ""), "meta": meta_in}
                return self._reply(200, [("Content-Type", "application/xml")],
                                   f"<InitiateMultipartUploadResult><UploadId>{uid}</UploadId></InitiateMultipartUploadResult>".encode())
            if self.command == "PUT" and "partNumber" in q:
                n = int(q["partNumber"]); uid = q.get("uploadId", "")
                if store.fail_part == n:
                    return self._error(500, "InternalError", "part refused")
                if uid not in store.uploads:
                    return self._error(404, "NoSuchUpload", "no such upload")
                store.uploads[uid]["parts"][n] = body
                return self._reply(200, [("ETag", f'"{hashlib.md5(body).hexdigest()}"')])
            if self.command == "POST" and "uploadId" in q:
                up = store.uploads.pop(q["uploadId"], None)
                if up is None:
                    return self._error(404, "NoSuchUpload", "no such upload")
                parts = [up["parts"][n] for n in sorted(up["parts"])]
                etag = multipart_etag([hashlib.md5(p).hexdigest() for p in parts])
                store.objects[(bucket, key)] = {"data": b"".join(parts), "ct": up["ct"], "meta": up["meta"], "etag": etag}
                return self._reply(200, [("Content-Type", "application/xml")],
                                   f'<CompleteMultipartUploadResult><ETag>"{etag}"</ETag></CompleteMultipartUploadResult>'.encode())
            if self.command == "DELETE" and "uploadId" in q:
                store.uploads.pop(q["uploadId"], None)
                return self._reply(204)
            if self.command == "PUT" and self.headers.get("x-amz-copy-source"):
                src = unquote(self.headers["x-amz-copy-source"]).lstrip("/").split("/", 1)
                o = store.objects.get((src[0], src[1]))
                if o is None:
                    return self._error(404, "NoSuchKey", "no source")
                store.objects[(bucket, key)] = dict(o)
                return self._reply(200, [("Content-Type", "application/xml")],
                                   f'<CopyObjectResult><ETag>"{o["etag"]}"</ETag></CopyObjectResult>'.encode())
            if self.command == "PUT":
                etag = "0" * 32 if key == "bad-etag" else hashlib.md5(body).hexdigest()
                store.objects[(bucket, key)] = {"data": body, "ct": self.headers.get("Content-Type", ""), "meta": meta_in, "etag": etag}
                return self._reply(200, [("ETag", f'"{etag}"')])
            if self.command == "DELETE":
                store.objects.pop((bucket, key), None)
                return self._reply(204)
            if (bucket, key) in store.objects:
                o = store.objects[(bucket, key)]
                headers = [("ETag", f'"{o["etag"]}"'), ("Content-Type", o["ct"]),
                           ("Last-Modified", "Thu, 23 Oct 2026 12:00:00 GMT"), ("Content-Length", str(len(o["data"])))]
                headers += [(f"x-amz-meta-{k}", v) for k, v in o["meta"].items()]
                return self._reply(200, headers, o["data"])
            return self._error(404, "NoSuchKey", "The specified key does not exist.")

    do_GET = do_PUT = do_HEAD = do_DELETE = do_POST = _handle


def start():
    store = Store()
    handler = type("H", (Handler,), {"store": store})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, store, f"http://127.0.0.1:{server.server_port}"
