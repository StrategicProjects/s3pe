//! Python bindings of the `s3pe` crate: a `Client` class, typed exceptions
//! and two helpers. Everything else (formats, logging) lives in Python.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use std::time::Duration;

create_exception!(_s3pe, S3Error, PyException, "Base class of every s3pe error.");
create_exception!(_s3pe, ConfigError, S3Error, "Missing credentials, bucket or a bad endpoint.");
create_exception!(_s3pe, TransportError, S3Error, "The server could not be reached (after retries).");
create_exception!(_s3pe, NotFoundError, S3Error, "The object or bucket does not exist (HTTP 404).");
create_exception!(_s3pe, AccessDeniedError, S3Error, "The credentials do not allow this (HTTP 403).");
create_exception!(_s3pe, HttpError, S3Error, "Another HTTP error; see .status, .code and .message.");
create_exception!(_s3pe, IntegrityError, S3Error, "What arrived is not what was sent.");

fn to_py(py: Python<'_>, e: s3pe::Error) -> PyErr {
    let text = e.to_string();
    match e {
        s3pe::Error::Config(_) => ConfigError::new_err(text),
        s3pe::Error::Transport(_) => TransportError::new_err(text),
        s3pe::Error::NotFound { .. } => NotFoundError::new_err(text),
        s3pe::Error::AccessDenied { .. } => AccessDeniedError::new_err(text),
        s3pe::Error::Http { status, code, message, .. } => {
            let err = HttpError::new_err(text);
            let v = err.value(py);
            let _ = v.setattr("status", status);
            let _ = v.setattr("code", code);
            let _ = v.setattr("message", message);
            err
        }
        s3pe::Error::Integrity(_) => IntegrityError::new_err(text),
        s3pe::Error::Io(io) => PyErr::new::<pyo3::exceptions::PyIOError, _>(io.to_string()),
    }
}

fn object_dict<'py>(py: Python<'py>, o: &s3pe::Object) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("key", &o.key)?;
    d.set_item("size", o.size)?;
    d.set_item("last_modified", &o.last_modified)?;
    d.set_item("etag", &o.etag)?;
    d.set_item("content_type", o.content_type.as_deref())?;
    Ok(d)
}

fn transfer_dict<'py>(py: Python<'py>, t: &s3pe::Transfer) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("bucket", &t.bucket)?;
    d.set_item("key", &t.key)?;
    d.set_item("size", t.size)?;
    d.set_item("etag", &t.etag)?;
    d.set_item("elapsed", t.elapsed.as_secs_f64())?;
    Ok(d)
}

/// The S3 client of the Rust crate. Arguments left as `None` come from the
/// environment, then from the defaults.
#[pyclass(frozen, name = "Client", module = "s3pe._s3pe")]
struct Client {
    inner: s3pe::Client,
}

#[pymethods]
impl Client {
    #[new]
    #[pyo3(signature = (endpoint=None, region=None, access_key=None, secret_key=None, session_token=None, bucket=None, timeout=None, max_tries=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        endpoint: Option<String>,
        region: Option<String>,
        access_key: Option<String>,
        secret_key: Option<String>,
        session_token: Option<String>,
        bucket: Option<String>,
        timeout: Option<f64>,
        max_tries: Option<u32>,
    ) -> PyResult<Self> {
        let mut b = s3pe::Client::builder();
        if let Some(v) = endpoint {
            b = b.endpoint(v);
        }
        if let Some(v) = region {
            b = b.region(v);
        }
        match (access_key, secret_key) {
            (Some(a), Some(s)) => b = b.credentials(a, s),
            (None, None) => {}
            _ => {
                return Err(ConfigError::new_err(
                    "access_key and secret_key must be given together",
                ))
            }
        }
        if let Some(v) = session_token {
            b = b.session_token(v);
        }
        if let Some(v) = bucket {
            b = b.bucket(v);
        }
        if let Some(v) = timeout {
            b = b.timeout(Duration::from_secs_f64(v.max(0.001)));
        }
        if let Some(v) = max_tries {
            b = b.max_tries(v);
        }
        Ok(Client { inner: b.build().map_err(|e| to_py(py, e))? })
    }

    /// The endpoint, as ``scheme://host[:port]``.
    #[getter]
    fn endpoint(&self) -> &str {
        self.inner.endpoint()
    }

    /// The default bucket, or ``None``.
    #[getter]
    fn bucket(&self) -> Option<&str> {
        self.inner.bucket()
    }

    /// ``(bucket, key)`` for a bare key or an ``s3://`` URL.
    fn resolve(&self, py: Python<'_>, key: &str) -> PyResult<(String, String)> {
        self.inner.resolve(key).map_err(|e| to_py(py, e))
    }

    /// The object's bytes.
    fn read<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, PyBytes>> {
        let data = py.detach(|| self.inner.read(key)).map_err(|e| to_py(py, e))?;
        Ok(PyBytes::new(py, &data))
    }

    /// Writes bytes; returns a dict with bucket, key, size, etag, elapsed.
    #[pyo3(signature = (key, data, content_type=None))]
    fn write<'py>(
        &self,
        py: Python<'py>,
        key: &str,
        data: &[u8],
        content_type: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let t = py
            .detach(|| self.inner.write(key, data, content_type))
            .map_err(|e| to_py(py, e))?;
        transfer_dict(py, &t)
    }

    /// Downloads to a file (streaming, written as ``.s3pe-part`` then renamed).
    fn download<'py>(&self, py: Python<'py>, key: &str, path: &str) -> PyResult<Bound<'py, PyDict>> {
        let t = py
            .detach(|| self.inner.download(key, path))
            .map_err(|e| to_py(py, e))?;
        transfer_dict(py, &t)
    }

    /// Uploads a file (streaming).
    #[pyo3(signature = (path, key, content_type=None))]
    fn upload<'py>(
        &self,
        py: Python<'py>,
        path: &str,
        key: &str,
        content_type: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let t = py
            .detach(|| self.inner.upload(path, key, content_type))
            .map_err(|e| to_py(py, e))?;
        transfer_dict(py, &t)
    }

    /// Every object under the prefix, as dicts (all pages).
    #[pyo3(signature = (prefix=""))]
    fn list<'py>(&self, py: Python<'py>, prefix: &str) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let objects = py.detach(|| self.inner.list(prefix)).map_err(|e| to_py(py, e))?;
        objects.iter().map(|o| object_dict(py, o)).collect()
    }

    /// Deletes an object (no error if it does not exist).
    fn delete(&self, py: Python<'_>, key: &str) -> PyResult<()> {
        py.detach(|| self.inner.delete(key)).map_err(|e| to_py(py, e))
    }

    /// Metadata dict, or ``None`` when the object does not exist.
    fn head<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Option<Bound<'py, PyDict>>> {
        match py.detach(|| self.inner.head(key)).map_err(|e| to_py(py, e))? {
            Some(o) => Ok(Some(object_dict(py, &o)?)),
            None => Ok(None),
        }
    }

    /// Whether the object exists.
    fn exists(&self, py: Python<'_>, key: &str) -> PyResult<bool> {
        py.detach(|| self.inner.exists(key)).map_err(|e| to_py(py, e))
    }

    /// The buckets the credentials can see, as ``(name, created)`` tuples.
    fn buckets(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        let b = py.detach(|| self.inner.buckets()).map_err(|e| to_py(py, e))?;
        Ok(b.into_iter().map(|b| (b.name, b.created)).collect())
    }

    fn __repr__(&self) -> String {
        format!(
            "Client(endpoint={:?}, bucket={:?})",
            self.inner.endpoint(),
            self.inner.bucket()
        )
    }
}

/// The content type the key's extension suggests.
#[pyfunction]
fn content_type_for(key: &str) -> &'static str {
    s3pe::content_type_for(key)
}

/// ``12.3 MB``, ``1.2 kB``, ``512 B``.
#[pyfunction]
fn format_size(n: u64) -> String {
    s3pe::format_size(n)
}

/// Signs a request (for tests and for callers who build their own HTTP
/// requests): returns the headers to add, as a dict.
#[pyfunction]
#[pyo3(signature = (method, host, path, query, payload_hash, amz_date, access_key, secret_key, region, session_token=None))]
#[allow(clippy::too_many_arguments)]
fn sign<'py>(
    py: Python<'py>,
    method: &str,
    host: &str,
    path: &str,
    query: Vec<(String, String)>,
    payload_hash: &str,
    amz_date: &str,
    access_key: String,
    secret_key: String,
    region: &str,
    session_token: Option<String>,
) -> PyResult<Bound<'py, PyDict>> {
    let creds = s3pe::sigv4::Credentials { access_key, secret_key, session_token };
    let headers = s3pe::sigv4::sign(
        &s3pe::sigv4::SignRequest {
            method,
            host,
            path,
            query: &query,
            payload_hash,
            amz_date,
            extra_headers: &[],
        },
        &creds,
        region,
    );
    let d = PyDict::new(py);
    for (k, v) in headers {
        d.set_item(k, v)?;
    }
    Ok(d)
}

#[pymodule]
fn _s3pe(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Client>()?;
    m.add_function(wrap_pyfunction!(content_type_for, m)?)?;
    m.add_function(wrap_pyfunction!(format_size, m)?)?;
    m.add_function(wrap_pyfunction!(sign, m)?)?;
    m.add("S3Error", m.py().get_type::<S3Error>())?;
    m.add("ConfigError", m.py().get_type::<ConfigError>())?;
    m.add("TransportError", m.py().get_type::<TransportError>())?;
    m.add("NotFoundError", m.py().get_type::<NotFoundError>())?;
    m.add("AccessDeniedError", m.py().get_type::<AccessDeniedError>())?;
    m.add("HttpError", m.py().get_type::<HttpError>())?;
    m.add("IntegrityError", m.py().get_type::<IntegrityError>())?;
    m.add("DEFAULT_ENDPOINT", s3pe::DEFAULT_ENDPOINT)?;
    m.add("DEFAULT_REGION", s3pe::DEFAULT_REGION)?;
    m.add("CRATE_VERSION", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
