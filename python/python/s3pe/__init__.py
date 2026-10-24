"""s3pe: read and write data on any S3-compatible object store.

Nine verbs over a Rust core: ``read_s3``, ``write_s3``, ``download_s3``,
``upload_s3``, ``list_s3``, ``delete_s3``, ``exists_s3``, ``head_s3`` and
``buckets_s3``, plus ``copy_s3``, ``presign_s3``, ``sync_up_s3`` and
``sync_down_s3``. Bodies above 64 MiB go up as a multipart upload. Endpoint and credentials come from the environment
(``S3PE_ENDPOINT``, ``AWS_ACCESS_KEY_ID``, ``AWS_SECRET_ACCESS_KEY``,
``S3PE_BUCKET``); a :class:`Client` takes them explicitly.

    >>> import s3pe
    >>> meta = s3pe.read_s3("s3://my-bucket/data/manifest.json")   # dict
    >>> df = s3pe.read_s3("etl/sales.parquet", bucket="my-bucket")  # DataFrame
    >>> s3pe.write_s3(df, "s3://my-bucket/reports/sales.csv")

The format follows the key's extension (``.json``, ``.csv``, ``.csv.gz``,
``.tsv``, ``.parquet``, ``.xlsx``, ``.geojson``, text files; anything else
is ``bytes``) or ``format=``. pandas is needed for tables
(``pip install s3pe[pandas]``).

What happens in every call:

.. image:: https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/request-flow.svg
   :alt: Resolve the key, hash the payload, sign with SigV4, send, retry on 429 and 5xx, verify the ETag and Content-Length; success logs one line, errors are typed.
"""

from __future__ import annotations

import gzip
import io
import json
import logging
import os
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional, Union

from . import _s3pe
from ._s3pe import (
    DEFAULT_REGION,
    AccessDeniedError,
    ConfigError,
    HttpError,
    IntegrityError,
    NotFoundError,
    S3Error,
    TransportError,
    content_type_for,
    format_size,
)

__version__ = _s3pe.CRATE_VERSION

__all__ = [
    "Client",
    "S3Object",
    "Bucket",
    "Transfer",
    "read_s3",
    "write_s3",
    "download_s3",
    "upload_s3",
    "list_s3",
    "delete_s3",
    "exists_s3",
    "head_s3",
    "buckets_s3",
    "copy_s3",
    "presign_s3",
    "sync_up_s3",
    "sync_down_s3",
    "SyncReport",
    "configure",
    "quiet",
    "content_type_for",
    "format_size",
    "format_for",
    "S3Error",
    "ConfigError",
    "TransportError",
    "NotFoundError",
    "AccessDeniedError",
    "HttpError",
    "IntegrityError",
    "DEFAULT_REGION",
    "__version__",
]

# --------------------------------------------------------------------------
# Logging: one line per transfer on stderr, silenced by quiet() or S3PE_QUIET.

log = logging.getLogger("s3pe")
_handler = logging.StreamHandler(sys.stderr)
_handler.setFormatter(logging.Formatter("%(message)s"))
log.addHandler(_handler)
log.propagate = False


def quiet(on: bool = True) -> None:
    """Silence (or restore) the one-line-per-transfer messages."""
    log.setLevel(logging.WARNING if on else logging.INFO)


quiet(os.environ.get("S3PE_QUIET", "") not in ("", "0"))


# --------------------------------------------------------------------------
# Records


def _parse_time(s: str) -> Optional[datetime]:
    if not s:
        return None
    for fmt in ("%Y-%m-%dT%H:%M:%S.%fZ", "%Y-%m-%dT%H:%M:%SZ", "%a, %d %b %Y %H:%M:%S GMT"):
        try:
            return datetime.strptime(s, fmt).replace(tzinfo=timezone.utc)
        except ValueError:
            continue
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return None


@dataclass(frozen=True)
class S3Object:
    """One object, as listed or described by the store. In a one-level
    listing (``delimiter="/"``) a common prefix ("folder") is an entry whose
    key ends with ``/`` and whose size, date and ETag are ``None``."""

    key: str
    size: Optional[int]
    last_modified: Optional[datetime]
    etag: Optional[str]
    content_type: Optional[str] = None
    metadata: Optional[dict] = None

    @property
    def is_prefix(self) -> bool:
        return self.size is None and self.key.endswith("/")

    @classmethod
    def _from(cls, d: dict) -> "S3Object":
        return cls(d["key"], d["size"], _parse_time(d["last_modified"]), d["etag"], d.get("content_type"), d.get("metadata") or {})

    @classmethod
    def _prefix(cls, p: str) -> "S3Object":
        return cls(p, None, None, None, None, None)


@dataclass(frozen=True)
class SyncReport:
    """What a sync did: relative paths transferred, skipped and deleted."""

    transferred: list
    skipped: list
    deleted: list
    elapsed: float

    @classmethod
    def _from(cls, d: dict) -> "SyncReport":
        return cls(d["transferred"], d["skipped"], d["deleted"], d["elapsed"])


@dataclass(frozen=True)
class Bucket:
    """A bucket, as listed by the store."""

    name: str
    created: Optional[datetime]


@dataclass(frozen=True)
class Transfer:
    """What a completed upload or download reports."""

    bucket: str
    key: str
    size: int
    etag: str
    elapsed: float

    @property
    def url(self) -> str:
        return f"s3://{self.bucket}/{self.key}"

    @classmethod
    def _from(cls, d: dict) -> "Transfer":
        return cls(d["bucket"], d["key"], d["size"], d["etag"], d["elapsed"])


# --------------------------------------------------------------------------
# Formats

_TEXT = {"txt", "md", "html", "htm", "xml", "svg", "yaml", "yml", "toml", "css", "js", "r", "py", "sql"}
_FORMATS = {"csv", "tsv", "json", "geojson", "parquet", "xlsx", "text", "bytes", "raw"}


def format_for(key: str) -> str:
    """The format a key's extension implies: ``csv``, ``tsv``, ``json``,
    ``geojson``, ``parquet``, ``xlsx``, ``text`` or ``bytes``. A ``.gz``
    suffix is looked through (``series.csv.gz`` is ``csv``)."""
    name = key.rsplit("/", 1)[-1].lower()
    if name.endswith(".gz"):
        name = name[:-3]
    ext = name.rsplit(".", 1)[-1] if "." in name else ""
    if ext in ("csv", "tsv", "json", "geojson", "parquet", "xlsx"):
        return ext
    if ext in _TEXT:
        return "text"
    return "bytes"


def _is_gz(key: str) -> bool:
    return key.lower().endswith(".gz")


def _pandas():
    try:
        import pandas  # type: ignore
    except ImportError as e:  # pragma: no cover
        raise ImportError("tables need pandas: pip install 's3pe[pandas]'") from e
    return pandas


def _decode(data: bytes, key: str, fmt: Optional[str], kwargs: dict) -> Any:
    fmt = (fmt or format_for(key)).lower()
    if fmt not in _FORMATS:
        raise ValueError(f"unknown format {fmt!r}; one of {sorted(_FORMATS)}")
    if fmt in ("bytes", "raw"):
        return data
    if _is_gz(key) and fmt != "bytes":
        data = gzip.decompress(data)
    if fmt == "text":
        return data.decode(kwargs.pop("encoding", "utf-8"))
    if fmt in ("json", "geojson"):
        return json.loads(data.decode(kwargs.pop("encoding", "utf-8")), **kwargs)
    pd = _pandas()
    if fmt == "csv":
        return pd.read_csv(io.BytesIO(data), **kwargs)
    if fmt == "tsv":
        kwargs.setdefault("sep", "\t")
        return pd.read_csv(io.BytesIO(data), **kwargs)
    if fmt == "parquet":
        return pd.read_parquet(io.BytesIO(data), **kwargs)
    if fmt == "xlsx":
        return pd.read_excel(io.BytesIO(data), **kwargs)
    raise AssertionError(fmt)


def _encode(obj: Any, key: str, fmt: Optional[str], kwargs: dict) -> bytes:
    if isinstance(obj, (bytes, bytearray, memoryview)):
        return bytes(obj)
    if hasattr(obj, "read") and callable(obj.read):
        data = obj.read()
        return data if isinstance(data, bytes) else str(data).encode("utf-8")
    fmt = (fmt or format_for(key)).lower()
    if fmt not in _FORMATS:
        raise ValueError(f"unknown format {fmt!r}; one of {sorted(_FORMATS)}")
    is_table = hasattr(obj, "to_csv") and hasattr(obj, "to_parquet")
    if isinstance(obj, str):
        if fmt in ("json", "geojson"):
            data = json.dumps(obj, **kwargs).encode("utf-8")
        else:
            data = obj.encode(kwargs.pop("encoding", "utf-8"))
    elif is_table:
        if fmt == "csv":
            kwargs.setdefault("index", False)
            data = obj.to_csv(**kwargs).encode("utf-8")
        elif fmt == "tsv":
            kwargs.setdefault("index", False)
            kwargs.setdefault("sep", "\t")
            data = obj.to_csv(**kwargs).encode("utf-8")
        elif fmt == "parquet":
            buf = io.BytesIO()
            kwargs.setdefault("index", False)
            obj.to_parquet(buf, **kwargs)
            data = buf.getvalue()
        elif fmt == "xlsx":
            buf = io.BytesIO()
            kwargs.setdefault("index", False)
            obj.to_excel(buf, **kwargs)
            data = buf.getvalue()
        elif fmt in ("json", "geojson"):
            kwargs.setdefault("orient", "records")
            data = obj.to_json(**kwargs).encode("utf-8")
        else:
            raise ValueError(f"cannot write a table as {fmt!r}: use .csv, .parquet, .xlsx or .json")
    elif fmt in ("json", "geojson") or isinstance(obj, (dict, list, tuple, int, float, bool)) or obj is None:
        kwargs.setdefault("ensure_ascii", False)
        kwargs.setdefault("default", str)
        data = json.dumps(obj, **kwargs).encode("utf-8")
    else:
        raise TypeError(f"cannot write an object of type {type(obj).__name__}: give bytes, str, dict, list or a DataFrame")
    if _is_gz(key):
        data = gzip.compress(data)
    return data


# --------------------------------------------------------------------------
# Client


def _pairs(d: Optional[dict]) -> Optional[list]:
    return None if d is None else [(str(k), str(v)) for k, v in d.items()]


def _with_bucket(key: str, bucket: Optional[str]) -> str:
    if bucket and not key.startswith("s3://"):
        return f"s3://{bucket}/{key.lstrip('/')}"
    return key


class Client:
    """A configured connection to one S3 endpoint.

    Every argument left as ``None`` comes from the environment
    (``S3PE_ENDPOINT``/``AWS_ENDPOINT_URL``, ``AWS_REGION``,
    ``AWS_ACCESS_KEY_ID``, ``AWS_SECRET_ACCESS_KEY``, ``AWS_SESSION_TOKEN``,
    ``S3PE_BUCKET``), then from the defaults (``us-east-1``, 300 s, 3
    tries; multipart above 64 MiB in 16 MiB parts, 4 at a time, via
    ``S3PE_MULTIPART_THRESHOLD``, ``S3PE_PART_SIZE``, ``S3PE_PARALLEL``). The
    endpoint and the credentials are required. Keys are relative to ``bucket`` or full
    ``s3://bucket/key`` URLs.
    """

    def __init__(
        self,
        endpoint: Optional[str] = None,
        region: Optional[str] = None,
        access_key: Optional[str] = None,
        secret_key: Optional[str] = None,
        session_token: Optional[str] = None,
        bucket: Optional[str] = None,
        timeout: Optional[float] = None,
        max_tries: Optional[int] = None,
        multipart_threshold: Optional[int] = None,
        part_size: Optional[int] = None,
        parallel: Optional[int] = None,
    ) -> None:
        self._inner = _s3pe.Client(
            endpoint=endpoint,
            region=region,
            access_key=access_key,
            secret_key=secret_key,
            session_token=session_token,
            bucket=bucket,
            timeout=timeout,
            max_tries=max_tries,
            multipart_threshold=multipart_threshold,
            part_size=part_size,
            parallel=parallel,
        )

    @property
    def endpoint(self) -> str:
        return self._inner.endpoint

    @property
    def bucket(self) -> Optional[str]:
        return self._inner.bucket

    def __repr__(self) -> str:
        return f"s3pe.Client(endpoint={self.endpoint!r}, bucket={self.bucket!r})"

    def resolve(self, key: str) -> tuple:
        """``(bucket, key)`` for a bare key or an ``s3://`` URL."""
        return self._inner.resolve(key)

    def url(self, key: str) -> str:
        b, k = self.resolve(key)
        return f"s3://{b}/{k}"

    # -- objects -----------------------------------------------------------

    def read_bytes(self, key: str) -> bytes:
        """The object's bytes, as they are."""
        import time

        t0 = time.perf_counter()
        data = self._inner.read(key)
        log.info("↓ %s (%s, %.1f s)", self.url(key), format_size(len(data)), time.perf_counter() - t0)
        return data

    def read(self, key: str, *, format: Optional[str] = None, **kwargs: Any) -> Any:
        """The object, decoded by its extension or ``format``; ``kwargs`` go
        to the reader (``pandas.read_csv``, ``json.loads``, ...)."""
        return _decode(self.read_bytes(key), key, format, kwargs)

    def write(self, obj: Any, key: str, *, format: Optional[str] = None, content_type: Optional[str] = None, metadata: Optional[dict] = None, headers: Optional[dict] = None, **kwargs: Any) -> Transfer:
        """Writes ``obj`` (bytes, str, dict/list, DataFrame, file-like or
        :class:`pathlib.Path`) to the object; ``kwargs`` go to the writer.
        ``metadata`` becomes ``x-amz-meta-*`` headers; ``headers`` are sent
        as they are (``Cache-Control``, ``x-amz-server-side-encryption``, ...)."""
        if isinstance(obj, Path):
            return self.upload(obj, key, content_type=content_type, metadata=metadata, headers=headers)
        data = _encode(obj, key, format, kwargs)
        t = Transfer._from(self._inner.write(key, data, content_type, _pairs(metadata), _pairs(headers)))
        log.info("↑ %s (%s, %.1f s)", t.url, format_size(t.size), t.elapsed)
        return t

    def download(self, key: str, path: Union[str, os.PathLike], *, overwrite: bool = False) -> Transfer:
        """Downloads the object to ``path`` (streaming; a directory gets the
        key's base name). Refuses to replace an existing file unless
        ``overwrite``."""
        p = Path(path)
        if p.is_dir():
            p = p / self.resolve(key)[1].rsplit("/", 1)[-1]
        if p.exists() and not overwrite:
            raise FileExistsError(f"{p} exists; pass overwrite=True")
        t = Transfer._from(self._inner.download(key, str(p)))
        log.info("↓ %s → %s (%s, %.1f s)", t.url, p, format_size(t.size), t.elapsed)
        return t

    def upload(self, path: Union[str, os.PathLike], key: Optional[str] = None, *, content_type: Optional[str] = None, metadata: Optional[dict] = None, headers: Optional[dict] = None) -> Transfer:
        """Uploads a file (streaming; multipart above the threshold). ``key``
        ``None`` or ending in ``/`` gets the file's name appended."""
        p = Path(path)
        k = key if key is not None else p.name
        t = Transfer._from(self._inner.upload(str(p), k, content_type, _pairs(metadata), _pairs(headers)))
        log.info("↑ %s → %s (%s, %.1f s)", p, t.url, format_size(t.size), t.elapsed)
        return t

    def list(self, prefix: str = "", *, delimiter: Optional[str] = None) -> list:
        """Every object under ``prefix`` (all pages), as :class:`S3Object`.
        With ``delimiter="/"`` only one level: the objects directly under the
        prefix plus the common prefixes, as entries with ``size None``."""
        import time

        t0 = time.perf_counter()
        if delimiter:
            if delimiter != "/":
                raise ValueError("only '/' is supported as delimiter")
            objs, prefixes = self._inner.list_dir(prefix)
            objects = [S3Object._prefix(p) for p in prefixes] + [S3Object._from(d) for d in objs]
        else:
            objects = [S3Object._from(d) for d in self._inner.list(prefix)]
        b, pre = self.resolve(prefix)
        n = len(objects)
        log.info("✔ %d object%s under s3://%s/%s (%.1f s)", n, "" if n == 1 else "s", b, pre, time.perf_counter() - t0)
        return objects

    def delete(self, key: Union[str, list, tuple]) -> None:
        """Deletes one object (``DELETE``) or several (``DeleteObjects``, 1000
        per request); deleting what does not exist is fine."""
        if isinstance(key, (list, tuple)):
            keys = [str(k) for k in key]
            if not keys:
                return
            self._inner.delete_many(keys)
            log.info("✖ %d objects", len(keys))
            return
        self._inner.delete(key)
        log.info("✖ %s", self.url(key))

    def copy(self, from_key: str, to_key: str) -> Transfer:
        """Server-side copy of an object (metadata kept)."""
        t = Transfer._from(self._inner.copy(from_key, to_key))
        log.info("⧉ %s → %s (%s, %.1f s)", self.url(from_key), t.url, format_size(t.size), t.elapsed)
        return t

    def presign(self, key: str, *, method: str = "GET", expires: float = 3600) -> str:
        """A URL that reads (or, with ``method="PUT"``, writes) the object
        without credentials for ``expires`` seconds (at most seven days)."""
        return self._inner.presign(key, method, float(expires))

    def sync_up(self, dir: Union[str, os.PathLike], prefix: str, *, delete: bool = False, last: Optional[str] = None) -> SyncReport:
        """Uploads every file under ``dir`` that is missing or differs (by
        ETag) under ``prefix``; ``last`` names the file to upload at the end;
        ``delete`` removes remote objects without a local file."""
        r = SyncReport._from(self._inner.sync_up(str(dir), prefix, delete, last))
        b, pre = self.resolve(prefix)
        log.info("✔ sync %s → s3://%s/%s: %d uploaded, %d unchanged, %d deleted (%.1f s)", dir, b, pre, len(r.transferred), len(r.skipped), len(r.deleted), r.elapsed)
        return r

    def sync_down(self, prefix: str, dir: Union[str, os.PathLike], *, delete: bool = False) -> SyncReport:
        """Downloads every object under ``prefix`` that is missing or differs
        in ``dir``; ``delete`` removes local files that are not remote."""
        r = SyncReport._from(self._inner.sync_down(prefix, str(dir), delete))
        b, pre = self.resolve(prefix)
        log.info("✔ sync s3://%s/%s → %s: %d downloaded, %d unchanged, %d deleted (%.1f s)", b, pre, dir, len(r.transferred), len(r.skipped), len(r.deleted), r.elapsed)
        return r

    def exists(self, key: str) -> bool:
        """Whether the object exists."""
        return self._inner.exists(key)

    def head(self, key: str) -> Optional[S3Object]:
        """Metadata of the object, or ``None`` when it does not exist."""
        d = self._inner.head(key)
        return None if d is None else S3Object._from(d)

    def buckets(self) -> list:
        """The buckets the credentials can see."""
        return [Bucket(name, _parse_time(created)) for name, created in self._inner.buckets()]


# --------------------------------------------------------------------------
# Module-level verbs over a default client

_default: Optional[Client] = None


def configure(**kwargs: Any) -> Client:
    """Builds the default client used by the module-level verbs, with the
    same arguments as :class:`Client`. Without a call, the first verb builds
    it from the environment."""
    global _default
    _default = Client(**kwargs)
    return _default


def _client() -> Client:
    global _default
    if _default is None:
        _default = Client()
    return _default


def read_s3(key: str, bucket: Optional[str] = None, *, format: Optional[str] = None, **kwargs: Any) -> Any:
    """Reads and decodes an object: ``read_s3("s3://my-bucket/data/manifest.json")``
    or ``read_s3("data/manifest.json", bucket="my-bucket")``."""
    return _client().read(_with_bucket(key, bucket), format=format, **kwargs)


def write_s3(obj: Any, key: str, bucket: Optional[str] = None, *, format: Optional[str] = None, content_type: Optional[str] = None, metadata: Optional[dict] = None, headers: Optional[dict] = None, **kwargs: Any) -> Transfer:
    """Encodes and writes an object (bytes, str, dict/list, DataFrame, Path)."""
    return _client().write(obj, _with_bucket(key, bucket), format=format, content_type=content_type, metadata=metadata, headers=headers, **kwargs)


def download_s3(key: str, path: Union[str, os.PathLike], bucket: Optional[str] = None, *, overwrite: bool = False) -> Transfer:
    """Downloads an object to a file."""
    return _client().download(_with_bucket(key, bucket), path, overwrite=overwrite)


def upload_s3(path: Union[str, os.PathLike], key: Optional[str] = None, bucket: Optional[str] = None, *, content_type: Optional[str] = None, metadata: Optional[dict] = None, headers: Optional[dict] = None) -> Transfer:
    """Uploads a file (multipart above the threshold)."""
    k = key if key is not None else Path(path).name
    return _client().upload(path, _with_bucket(k, bucket), content_type=content_type, metadata=metadata, headers=headers)


def list_s3(prefix: str = "", bucket: Optional[str] = None, *, delimiter: Optional[str] = None) -> list:
    """Every object under a prefix; with ``delimiter="/"``, one level."""
    return _client().list(_with_bucket(prefix, bucket), delimiter=delimiter)


def delete_s3(key: Union[str, list, tuple], bucket: Optional[str] = None) -> None:
    """Deletes one object or a list of objects."""
    if isinstance(key, (list, tuple)):
        _client().delete([_with_bucket(k, bucket) for k in key])
    else:
        _client().delete(_with_bucket(key, bucket))


def copy_s3(from_key: str, to_key: str, bucket: Optional[str] = None) -> Transfer:
    """Copies an object inside the store."""
    return _client().copy(_with_bucket(from_key, bucket), _with_bucket(to_key, bucket))


def presign_s3(key: str, bucket: Optional[str] = None, *, method: str = "GET", expires: float = 3600) -> str:
    """A pre-signed URL for the object."""
    return _client().presign(_with_bucket(key, bucket), method=method, expires=expires)


def sync_up_s3(dir: Union[str, os.PathLike], prefix: str, bucket: Optional[str] = None, *, delete: bool = False, last: Optional[str] = None) -> SyncReport:
    """Mirrors a directory to a prefix."""
    return _client().sync_up(dir, _with_bucket(prefix, bucket), delete=delete, last=last)


def sync_down_s3(prefix: str, dir: Union[str, os.PathLike], bucket: Optional[str] = None, *, delete: bool = False) -> SyncReport:
    """Mirrors a prefix to a directory."""
    return _client().sync_down(_with_bucket(prefix, bucket), dir, delete=delete)


def exists_s3(key: str, bucket: Optional[str] = None) -> bool:
    """Whether an object exists."""
    return _client().exists(_with_bucket(key, bucket))


def head_s3(key: str, bucket: Optional[str] = None) -> Optional[S3Object]:
    """Metadata of an object, or ``None``."""
    return _client().head(_with_bucket(key, bucket))


def buckets_s3() -> list:
    """The buckets the credentials can see."""
    return _client().buckets()
