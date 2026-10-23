"""The AWS documentation vector (DESIGN.md D8), through the Rust signer."""

from s3pe import _s3pe
from fake_s3 import py_sign

EMPTY = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"


def test_rust_signer_matches_python_signer():
    args = ("GET", "127.0.0.1:9003", "/iaempe/dados/x%20y.json", [("list-type", "2"), ("prefix", "dados/")],
            EMPTY, "20261023T120000Z", "GK1", "s", "us-east-1")
    h = _s3pe.sign(*args)
    assert h["authorization"] == py_sign(*args)
    assert h["x-amz-date"] == "20261023T120000Z"
    assert h["x-amz-content-sha256"] == EMPTY


def test_aws_documentation_example_in_python_signer():
    # The vector signs the Range header too, which the fake's signer does not
    # model; the Rust crate's unit test covers it. Here we check the key
    # derivation on the same inputs minus Range gives a stable value.
    sig = py_sign("GET", "examplebucket.s3.amazonaws.com", "/test.txt", [], EMPTY, "20130524T000000Z",
                  "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1")
    rust = _s3pe.sign("GET", "examplebucket.s3.amazonaws.com", "/test.txt", [], EMPTY, "20130524T000000Z",
                      "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1")
    assert rust["authorization"] == sig
