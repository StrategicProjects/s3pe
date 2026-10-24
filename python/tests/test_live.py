"""Against a real store, only with S3PE_ENDPOINT set and S3PE_TEST_BUCKET naming a writable bucket."""

import json
import os

import pytest

import s3pe

pytestmark = pytest.mark.skipif(not (os.environ.get("S3PE_TEST_BUCKET") and os.environ.get("S3PE_ENDPOINT")), reason="S3PE_TEST_BUCKET / S3PE_ENDPOINT not set")


def test_live_round_trip(tmp_path):
    bucket = os.environ["S3PE_TEST_BUCKET"]
    prefix = f"s3pe-test/py-{os.getpid()}/"
    key = f"{prefix}ação e espaço.json"
    import urllib.request

    c = s3pe.Client(bucket=bucket, multipart_threshold=6 * 1024 * 1024, part_size=5 * 1024 * 1024)
    assert not c.exists(key)
    c.write({"ok": True, "n": [1, 2]}, key, metadata={"origin": "live"})
    assert c.read(key) == {"ok": True, "n": [1, 2]}
    h = c.head(key)
    assert h.content_type == "application/json" and h.metadata == {"origin": "live"}
    big = tmp_path / "big.bin"
    big.write_bytes(bytes(i % 253 for i in range(12 * 1024 * 1024)))
    t = c.upload(big, f"{prefix}big.bin")
    assert t.etag.endswith("-3")
    c.download(f"{prefix}big.bin", tmp_path / "back.bin")
    assert (tmp_path / "back.bin").read_bytes() == big.read_bytes()
    c.copy(key, f"{prefix}sub/copy.json")
    one = c.list(prefix, delimiter="/")
    assert [o.key for o in one if o.is_prefix] == [f"{prefix}sub/"]
    with urllib.request.urlopen(c.presign(key, expires=300)) as r:
        assert json.loads(r.read()) == {"ok": True, "n": [1, 2]}
    d = tmp_path / "sync"
    (d / "sub").mkdir(parents=True)
    (d / "a.txt").write_text("a")
    (d / "sub" / "b.txt").write_text("b")
    (d / "manifest.json").write_text("{}")
    r = c.sync_up(d, f"{prefix}sync/", last="manifest.json")
    assert r.transferred[-1] == "manifest.json" and len(r.transferred) == 3
    assert len(c.sync_up(d, f"{prefix}sync/").skipped) == 3
    r = c.sync_down(f"{prefix}sync/", tmp_path / "sync-back")
    assert len(r.transferred) == 3
    keys = [o.key for o in c.list(prefix)]
    assert len(keys) == 6
    c.delete(keys)
    assert c.list(prefix) == []
