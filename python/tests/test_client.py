import json
import os
import time

import pytest

import s3pe


def test_round_trip(fake):
    assert not s3pe.exists_s3("dados/metadata.json")
    assert s3pe.head_s3("dados/metadata.json") is None
    with pytest.raises(s3pe.NotFoundError):
        s3pe.read_s3("dados/metadata.json")

    t = s3pe.write_s3({"ultimo_mes": 202608}, "dados/metadata.json")
    assert t.url == "s3://test/dados/metadata.json"
    assert s3pe.read_s3("dados/metadata.json") == {"ultimo_mes": 202608}
    assert s3pe.read_s3("s3://test/dados/metadata.json", format="bytes") == b'{"ultimo_mes": 202608}'
    assert s3pe.read_s3("dados/metadata.json", bucket="test", format="text") == '{"ultimo_mes": 202608}'

    h = s3pe.head_s3("dados/metadata.json")
    assert h.size == 22 and h.content_type == "application/json" and len(h.etag) == 32
    assert h.last_modified.year == 2026
    assert s3pe.exists_s3("dados/metadata.json")
    s3pe.delete_s3("dados/metadata.json")
    assert not s3pe.exists_s3("dados/metadata.json")
    s3pe.delete_s3("dados/metadata.json")
    assert [b.name for b in s3pe.buckets_s3()] == ["test"]


def test_formats_and_special_keys(fake):
    key = "pasta/ação & x+y (1).txt"
    s3pe.write_s3("olá\n", key)
    assert s3pe.read_s3(key) == "olá\n"
    assert fake.objects[("test", key)]["ct"] == "text/plain"

    s3pe.write_s3(b"\x00\x01", "bin/blob")
    assert s3pe.read_s3("bin/blob") == b"\x00\x01"

    s3pe.write_s3([1, 2, {"a": "é"}], "x.json.gz")
    raw = s3pe.read_s3("x.json.gz", format="bytes")
    assert raw[:2] == b"\x1f\x8b"
    assert s3pe.read_s3("x.json.gz") == [1, 2, {"a": "é"}]
    assert fake.objects[("test", "x.json.gz")]["ct"] == "application/gzip"

    listed = s3pe.list_s3("pasta/")
    assert [o.key for o in listed] == [key]
    assert listed[0].size == len("olá\n".encode())


def test_listing_pages(fake):
    for i in range(5):
        s3pe.write_s3(str(i), f"p/{i}.txt")
    s3pe.write_s3("x", "q/other.txt")
    assert [o.key for o in s3pe.list_s3("p/")] == [f"p/{i}.txt" for i in range(5)]
    assert len(s3pe.list_s3()) == 6
    assert len(s3pe.list_s3("s3://test")) == 6
    assert s3pe.list_s3("nothing/") == []


def test_retries_and_errors(fake):
    fake.fail_first = 1
    s3pe.write_s3("ok", "flaky.txt")
    assert s3pe.read_s3("flaky.txt") == "ok"
    fake.fail_first = 5
    with pytest.raises(s3pe.HttpError) as e:
        s3pe.read_s3("flaky.txt")
    assert e.value.status == 503 and e.value.code == "SlowDown"
    n = len(fake.requests)
    with pytest.raises(s3pe.AccessDeniedError):
        s3pe.read_s3("s3://forbidden/x")
    assert len(fake.requests) == n + 1
    with pytest.raises(s3pe.IntegrityError):
        s3pe.write_s3(b"data", "bad-etag")
    with pytest.raises(s3pe.ConfigError):
        s3pe.Client(endpoint="not-a-url", access_key="a", secret_key="b")
    with pytest.raises(s3pe.ConfigError):
        s3pe.Client(access_key="a")
    monkeypatch = None  # endpoint is required: without S3PE_ENDPOINT the client cannot be built
    import os
    saved = os.environ.pop("S3PE_ENDPOINT")
    try:
        with pytest.raises(s3pe.ConfigError):
            s3pe.Client(access_key="a", secret_key="b")
    finally:
        os.environ["S3PE_ENDPOINT"] = saved
    assert isinstance(s3pe.NotFoundError(), s3pe.S3Error)


def test_files(fake, tmp_path):
    big = bytes(i % 251 for i in range(3_000_000))
    src = tmp_path / "painel.parquet"
    src.write_bytes(big)
    t = s3pe.upload_s3(src, "etl/")
    assert t.key == "etl/painel.parquet" and t.size == len(big)
    assert fake.objects[("test", "etl/painel.parquet")]["ct"] == "application/vnd.apache.parquet"
    t = s3pe.upload_s3(src)
    assert t.key == "painel.parquet"
    s3pe.write_s3(src, "etl/again.parquet")
    assert s3pe.exists_s3("etl/again.parquet")

    dest = tmp_path / "sub" / "copy.parquet"
    t = s3pe.download_s3("etl/painel.parquet", dest)
    assert dest.read_bytes() == big and t.size == len(big)
    assert not (tmp_path / "sub" / "copy.parquet.s3pe-part").exists()
    with pytest.raises(FileExistsError):
        s3pe.download_s3("etl/painel.parquet", dest)
    s3pe.download_s3("etl/painel.parquet", dest, overwrite=True)
    (tmp_path / "dir").mkdir()
    s3pe.download_s3("etl/painel.parquet", tmp_path / "dir")
    assert (tmp_path / "dir" / "painel.parquet").read_bytes() == big
    with pytest.raises(s3pe.NotFoundError):
        s3pe.download_s3("etl/missing.parquet", tmp_path / "missing")
    assert not (tmp_path / "missing").exists()


def test_client_object_and_configure(fake):
    c = s3pe.Client(bucket="test")
    assert "test" in repr(c) and c.endpoint.startswith("http://127.0.0.1")
    c.write("a,b\n1,2\n", "t.csv")
    assert c.read("t.csv", format="text") == "a,b\n1,2\n"
    assert c.resolve("s3://other/k") == ("other", "k")
    s3pe.configure(bucket="test", max_tries=1)
    assert s3pe.read_s3("t.csv", format="bytes") == b"a,b\n1,2\n"


def test_helpers():
    assert s3pe.format_size(12_345_678) == "12.3 MB"
    assert s3pe.content_type_for("x.csv.gz") == "application/gzip"
    assert s3pe.format_for("series/estado/icms.csv.gz") == "csv"
    assert s3pe.format_for("a/b.GeoJSON") == "geojson"
    assert s3pe.format_for("README.md") == "text"
    assert s3pe.format_for("blob") == "bytes"
    with pytest.raises(ValueError):
        s3pe._decode(b"", "x", "nope", {})
    with pytest.raises(TypeError):
        s3pe._encode(object(), "x.bin", None, {})


pandas = pytest.importorskip("pandas")


def test_tables(fake):
    df = pandas.DataFrame({"competencia": ["2026-07-01", "2026-08-01"], "valor": [1.5, 2.0]})
    s3pe.write_s3(df, "series/icms.csv.gz")
    back = s3pe.read_s3("series/icms.csv.gz")
    assert list(back.columns) == ["competencia", "valor"] and back["valor"].tolist() == [1.5, 2.0]
    s3pe.write_s3(df, "series/icms.tsv")
    assert s3pe.read_s3("series/icms.tsv").shape == (2, 2)
    s3pe.write_s3(df, "series/icms.json")
    assert s3pe.read_s3("series/icms.json") == json.loads(df.to_json(orient="records"))
    try:
        import pyarrow  # noqa: F401
    except ImportError:
        return
    s3pe.write_s3(df, "series/icms.parquet")
    assert s3pe.read_s3("series/icms.parquet").equals(df)


def test_multipart_copy_delete_many_list_dir_metadata(fake, tmp_path):
    c = s3pe.Client(bucket="test", multipart_threshold=1024 * 1024, part_size=5 * 1024 * 1024, parallel=3)
    big = bytes(i % 253 for i in range(11 * 1024 * 1024 + 7))
    t = c.write(big, "big/mem.bin", metadata={"origin": "test"}, headers={"Cache-Control": "max-age=60"})
    assert t.size == len(big) and t.etag.endswith("-3")
    assert fake.objects[("test", "big/mem.bin")]["data"] == big
    assert fake.objects[("test", "big/mem.bin")]["meta"] == {"origin": "test"}
    assert sum("partNumber" in r for r in fake.requests) == 3
    assert c.head("big/mem.bin").metadata == {"origin": "test"}
    f = tmp_path / "big.bin"
    f.write_bytes(big)
    t = c.upload(f, "big/file.bin")
    assert t.etag.endswith("-3") and fake.objects[("test", "big/file.bin")]["data"] == big
    assert s3pe._s3pe.expected_etag(str(f), 1024 * 1024, 5 * 1024 * 1024) == t.etag

    fake.fail_part = 2
    once = s3pe.Client(bucket="test", multipart_threshold=1024 * 1024, part_size=5 * 1024 * 1024, max_tries=1)
    with pytest.raises(s3pe.HttpError):
        once.write(big, "big/fails.bin")
    assert not fake.uploads and ("test", "big/fails.bin") not in fake.objects
    fake.fail_part = None

    c.write("1", "a/one.txt", metadata={"Author": "ana"})
    t = c.copy("a/one.txt", "s3://test/b/one-copy.txt")
    assert t.key == "b/one-copy.txt" and c.read("b/one-copy.txt") == "1"
    assert c.head("b/one-copy.txt").metadata == {"author": "ana"}
    with pytest.raises(s3pe.NotFoundError):
        c.copy("a/missing", "b/x")

    for i in range(5):
        c.write("x", f"d/{i}.txt")
    c.write("x", "d/sub/deep.txt")
    one = c.list("d/", delimiter="/")
    assert [o.key for o in one if o.is_prefix] == ["d/sub/"]
    assert len([o for o in one if not o.is_prefix]) == 5
    keys = [o.key for o in c.list("d/")]
    assert len(keys) == 6
    c.delete(keys)
    assert c.list("d/") == []
    assert sum("?delete" in r for r in fake.requests) == 1
    c.write("x", "undeletable")
    with pytest.raises(s3pe.HttpError) as e:
        s3pe.delete_s3(["undeletable", "a/one.txt"], bucket="test")
    assert e.value.code == "DeleteErrors"
    assert not c.exists("a/one.txt")


def test_presign_and_sync(fake, tmp_path):
    import urllib.request

    s3pe.write_s3(b"%PDF", "pub/report.pdf")
    url = s3pe.presign_s3("pub/report.pdf", expires=600)
    assert "X-Amz-Expires=600" in url and url.startswith(os.environ["S3PE_ENDPOINT"] + "/test/pub/report.pdf?")
    with urllib.request.urlopen(url) as r:
        assert r.read() == b"%PDF"
    with pytest.raises(urllib.error.HTTPError) as e:
        urllib.request.urlopen(url.replace("report.pdf", "other.pdf"))
    assert e.value.code == 403
    with pytest.raises(s3pe.ConfigError):
        s3pe.presign_s3("k", method="DELETE")

    d = tmp_path / "out"
    (d / "sub").mkdir(parents=True)
    (d / "a.csv").write_text("a")
    (d / "sub" / "b.csv").write_text("b")
    (d / "manifest.json").write_text("{}")
    r = s3pe.sync_up_s3(d, "s3://test/out/", delete=True, last="manifest.json")
    assert r.transferred == ["a.csv", "sub/b.csv", "manifest.json"]
    r = s3pe.sync_up_s3(d, "out/", delete=True, last="manifest.json")
    assert r.transferred == [] and len(r.skipped) == 3
    (d / "a.csv").write_text("a2")
    s3pe.write_s3("old", "out/stale.txt")
    r = s3pe.sync_up_s3(d, "out/", delete=True)
    assert r.transferred == ["a.csv"] and r.deleted == ["stale.txt"]
    with pytest.raises(s3pe.ConfigError):
        s3pe.sync_up_s3(d, "out/", last="nope")

    down = tmp_path / "down"
    down.mkdir()
    (down / "extra.txt").write_text("x")
    r = s3pe.sync_down_s3("out/", down, delete=True)
    assert len(r.transferred) == 3 and r.deleted == ["extra.txt"]
    assert (down / "sub" / "b.csv").read_text() == "b" and (down / "a.csv").read_text() == "a2"
    assert len(s3pe.sync_down_s3("out/", down).skipped) == 3
