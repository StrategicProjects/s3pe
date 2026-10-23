"""Against s3.pe.gov.br, only with S3PE_TEST_BUCKET set to a writable bucket."""

import os

import pytest

import s3pe

pytestmark = pytest.mark.skipif(not os.environ.get("S3PE_TEST_BUCKET"), reason="S3PE_TEST_BUCKET not set")


def test_live_round_trip(tmp_path):
    bucket = os.environ["S3PE_TEST_BUCKET"]
    prefix = f"s3pe-test/py-{os.getpid()}/"
    key = f"{prefix}ação e espaço.json"
    c = s3pe.Client(bucket=bucket)
    assert not c.exists(key)
    c.write({"ok": True, "n": [1, 2]}, key)
    assert c.read(key) == {"ok": True, "n": [1, 2]}
    assert c.head(key).content_type == "application/json"
    big = tmp_path / "big.bin"
    big.write_bytes(bytes(i % 253 for i in range(2_500_000)))
    c.upload(big, f"{prefix}big.bin")
    c.download(f"{prefix}big.bin", tmp_path / "back.bin")
    assert (tmp_path / "back.bin").read_bytes() == big.read_bytes()
    listed = c.list(prefix)
    assert {o.key for o in listed} == {key, f"{prefix}big.bin"}
    for o in listed:
        c.delete(o.key)
    assert c.list(prefix) == []
