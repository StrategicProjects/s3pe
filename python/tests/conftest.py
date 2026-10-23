import pytest

from fake_s3 import ACCESS, SECRET, start


@pytest.fixture
def fake(monkeypatch):
    server, store, url = start()
    monkeypatch.setenv("AWS_ACCESS_KEY_ID", ACCESS)
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", SECRET)
    monkeypatch.setenv("AWS_REGION", "us-east-1")
    monkeypatch.setenv("S3PE_ENDPOINT", url)
    monkeypatch.setenv("S3PE_BUCKET", "test")
    monkeypatch.delenv("AWS_SESSION_TOKEN", raising=False)
    import s3pe

    s3pe._default = None
    yield store
    server.shutdown()
    s3pe._default = None
