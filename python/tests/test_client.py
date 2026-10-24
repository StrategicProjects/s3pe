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
    assert fake.objects[("test", key)][1] == "text/plain"

    s3pe.write_s3(b"\x00\x01", "bin/blob")
    assert s3pe.read_s3("bin/blob") == b"\x00\x01"

    s3pe.write_s3([1, 2, {"a": "é"}], "x.json.gz")
    raw = s3pe.read_s3("x.json.gz", format="bytes")
    assert raw[:2] == b"\x1f\x8b"
    assert s3pe.read_s3("x.json.gz") == [1, 2, {"a": "é"}]
    assert fake.objects[("test", "x.json.gz")][1] == "application/gzip"

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
    assert fake.objects[("test", "etl/painel.parquet")][1] == "application/vnd.apache.parquet"
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
