//! Python bindings of the `s3pe` crate: a `Client` class, typed exceptions
//! and two helpers. Everything else (formats, logging) lives in Python.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
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
    let m = PyDict::new(py);
    for (k, v) in &o.metadata {
        m.set_item(k, v)?;
    }
    d.set_item("metadata", m)?;
    Ok(d)
}

fn put_options(content_type: Option<&str>, metadata: Option<Vec<(String, String)>>, headers: Option<Vec<(String, String)>>) -> s3pe::PutOptions {
    s3pe::PutOptions {
        content_type: content_type.map(str::to_string),
        metadata: metadata.unwrap_or_default(),
        headers: headers.unwrap_or_default(),
    }
}

fn sync_dict<'py>(py: Python<'py>, r: &s3pe::SyncReport) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("transferred", PyList::new(py, &r.transferred)?)?;
    d.set_item("skipped", PyList::new(py, &r.skipped)?)?;
    d.set_item("deleted", PyList::new(py, &r.deleted)?)?;
    d.set_item("elapsed", r.elapsed.as_secs_f64())?;
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
    #[pyo3(signature = (endpoint=None, region=None, access_key=None, secret_key=None, session_token=None, bucket=None, timeout=None, max_tries=None, multipart_threshold=None, part_size=None, parallel=None))]
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
        multipart_threshold: Option<u64>,
        part_size: Option<u64>,
        parallel: Option<usize>,
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
        if let Some(v) = multipart_threshold {
            b = b.multipart_threshold(v);
        }
        if let Some(v) = part_size {
            b = b.part_size(v);
        }
        if let Some(v) = parallel {
            b = b.parallel(v);
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
    #[pyo3(signature = (key, data, content_type=None, metadata=None, headers=None))]
    fn write<'py>(
        &self,
        py: Python<'py>,
        key: &str,
        data: &[u8],
        content_type: Option<&str>,
        metadata: Option<Vec<(String, String)>>,
        headers: Option<Vec<(String, String)>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let opts = put_options(content_type, metadata, headers);
        let t = py
            .detach(|| self.inner.write_with(key, data, &opts))
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

    /// Uploads a file (streaming; multipart above the threshold).
    #[pyo3(signature = (path, key, content_type=None, metadata=None, headers=None))]
    fn upload<'py>(
        &self,
        py: Python<'py>,
        path: &str,
        key: &str,
        content_type: Option<&str>,
        metadata: Option<Vec<(String, String)>>,
        headers: Option<Vec<(String, String)>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let opts = put_options(content_type, metadata, headers);
        let t = py
            .detach(|| self.inner.upload_with(path, key, &opts))
            .map_err(|e| to_py(py, e))?;
        transfer_dict(py, &t)
    }

    /// One level: ``(objects, prefixes)``.
    #[pyo3(signature = (prefix=""))]
    fn list_dir<'py>(&self, py: Python<'py>, prefix: &str) -> PyResult<(Vec<Bound<'py, PyDict>>, Vec<String>)> {
        let l = py.detach(|| self.inner.list_dir(prefix)).map_err(|e| to_py(py, e))?;
        let objects: PyResult<Vec<_>> = l.objects.iter().map(|o| object_dict(py, o)).collect();
        Ok((objects?, l.prefixes))
    }

    /// Server-side copy.
    fn copy<'py>(&self, py: Python<'py>, from: &str, to: &str) -> PyResult<Bound<'py, PyDict>> {
        let t = py.detach(|| self.inner.copy(from, to)).map_err(|e| to_py(py, e))?;
        transfer_dict(py, &t)
    }

    /// Deletes several objects in batches of 1000.
    fn delete_many(&self, py: Python<'_>, keys: Vec<String>) -> PyResult<()> {
        py.detach(|| self.inner.delete_many(&keys)).map_err(|e| to_py(py, e))
    }

    /// A pre-signed URL (GET or PUT) valid for ``expires`` seconds.
    #[pyo3(signature = (key, method="GET", expires=3600.0))]
    fn presign(&self, py: Python<'_>, key: &str, method: &str, expires: f64) -> PyResult<String> {
        self.inner
            .presign(key, method, Duration::from_secs_f64(expires.max(1.0)))
            .map_err(|e| to_py(py, e))
    }

    /// Uploads a directory to a prefix; returns the report dict.
    #[pyo3(signature = (dir, prefix, delete=false, last=None))]
    fn sync_up<'py>(&self, py: Python<'py>, dir: &str, prefix: &str, delete: bool, last: Option<String>) -> PyResult<Bound<'py, PyDict>> {
        let opts = s3pe::SyncOptions { delete, last };
        let r = py.detach(|| self.inner.sync_up(dir, prefix, &opts)).map_err(|e| to_py(py, e))?;
        sync_dict(py, &r)
    }

    /// Downloads a prefix to a directory; returns the report dict.
    #[pyo3(signature = (prefix, dir, delete=false))]
    fn sync_down<'py>(&self, py: Python<'py>, prefix: &str, dir: &str, delete: bool) -> PyResult<Bound<'py, PyDict>> {
        let opts = s3pe::SyncOptions { delete, last: None };
        let r = py.detach(|| self.inner.sync_down(prefix, dir, &opts)).map_err(|e| to_py(py, e))?;
        sync_dict(py, &r)
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

/// The ETag the store would give a local file: MD5, or the multipart ETag
/// above ``threshold`` with parts of ``part_size``.
#[pyfunction]
fn expected_etag(path: &str, threshold: u64, part_size: u64) -> PyResult<String> {
    s3pe::expected_etag(std::path::Path::new(path), threshold, part_size)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))
}

/// The query string of a pre-signed URL (for tests): the X-Amz-* parameters and the signature.
#[pyfunction]
#[pyo3(signature = (method, host, path, query, amz_date, expires, access_key, secret_key, region, session_token=None))]
#[allow(clippy::too_many_arguments)]
fn presign_query(
    method: &str,
    host: &str,
    path: &str,
    query: Vec<(String, String)>,
    amz_date: &str,
    expires: u64,
    access_key: String,
    secret_key: String,
    region: &str,
    session_token: Option<String>,
) -> String {
    let creds = s3pe::sigv4::Credentials { access_key, secret_key, session_token };
    s3pe::sigv4::presign(
        &s3pe::sigv4::PresignRequest { method, host, path, query: &query, amz_date, expires },
        &creds,
        region,
    )
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
    m.add_function(wrap_pyfunction!(expected_etag, m)?)?;
    m.add_function(wrap_pyfunction!(presign_query, m)?)?;
    m.add("S3Error", m.py().get_type::<S3Error>())?;
    m.add("ConfigError", m.py().get_type::<ConfigError>())?;
    m.add("TransportError", m.py().get_type::<TransportError>())?;
    m.add("NotFoundError", m.py().get_type::<NotFoundError>())?;
    m.add("AccessDeniedError", m.py().get_type::<AccessDeniedError>())?;
    m.add("HttpError", m.py().get_type::<HttpError>())?;
    m.add("IntegrityError", m.py().get_type::<IntegrityError>())?;
    m.add("DEFAULT_REGION", s3pe::DEFAULT_REGION)?;
    m.add("CRATE_VERSION", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
