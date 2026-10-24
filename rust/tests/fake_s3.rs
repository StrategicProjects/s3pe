//! The client against a fake S3 server on localhost that checks every
//! signature with the crate's own signer (header and query styles), stores
//! objects and metadata in memory, supports multipart uploads, copies,
//! batch deletes and one-level listings, and can misbehave on demand.

use md5::{Digest, Md5};
use s3pe::sigv4::{presign, sign, Credentials, PresignRequest, SignRequest};
use s3pe::{Client, Error, PutOptions, SyncOptions};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ACCESS: &str = "GKtest";
const SECRET: &str = "secret-of-the-fake";
const PAGE: usize = 2;

#[derive(Clone)]
struct Stored {
    data: Vec<u8>,
    content_type: String,
    metadata: BTreeMap<String, String>,
    etag: String,
}

#[derive(Default)]
struct Store {
    objects: BTreeMap<(String, String), Stored>,
    uploads: BTreeMap<String, BTreeMap<usize, Vec<u8>>>,
    requests: Vec<String>,
    fail_first: u32,
    fail_part: Option<usize>,
    next_upload: usize,
}

struct Fake {
    url: String,
    store: Arc<Mutex<Store>>,
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(h, 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn start() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Arc::new(Mutex::new(Store::default()));
    let st = store.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let st = st.clone();
            let addr = addr.to_string();
            std::thread::spawn(move || handle(conn.unwrap(), st, &addr));
        }
    });
    Fake {
        url: format!("http://{addr}"),
        store,
    }
}

fn creds() -> Credentials {
    Credentials {
        access_key: ACCESS.into(),
        secret_key: SECRET.into(),
        session_token: None,
    }
}

fn md5hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

fn handle(mut conn: TcpStream, store: Arc<Mutex<Store>>, host: &str) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 {
        return;
    }
    let mut parts = line.trim_end().split(' ');
    let method = parts.next().unwrap().to_string();
    let target = parts.next().unwrap().to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut l = String::new();
        reader.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    let hdr = |n: &str| {
        headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let len: usize = hdr("content-length").parse().unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();

    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect();
    let q = |n: &str| query.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());

    // Signature check: header style, or query style for pre-signed URLs.
    assert_eq!(hdr("host"), host, "host header");
    let ok = if let Some(sig) = q("X-Amz-Signature") {
        let plain: Vec<(String, String)> = query
            .iter()
            .filter(|(k, _)| !k.starts_with("X-Amz-"))
            .cloned()
            .collect();
        let expected = presign(
            &PresignRequest {
                method: &method,
                host,
                path,
                query: &plain,
                amz_date: &q("X-Amz-Date").unwrap(),
                expires: q("X-Amz-Expires").unwrap().parse().unwrap(),
            },
            &creds(),
            "us-east-1",
        );
        expected.ends_with(&format!("X-Amz-Signature={sig}"))
    } else {
        let payload_hash = hdr("x-amz-content-sha256");
        assert_eq!(payload_hash, s3pe::sigv4::sha256_hex(&body), "payload hash");
        let extra: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| {
                k.starts_with("x-amz-") && k != "x-amz-date" && k != "x-amz-content-sha256"
            })
            .cloned()
            .collect();
        let expected = sign(
            &SignRequest {
                method: &method,
                host,
                path,
                query: &query,
                payload_hash: &payload_hash,
                amz_date: &hdr("x-amz-date"),
                extra_headers: &extra,
            },
            &creds(),
            "us-east-1",
        );
        let auth = expected
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1
            .clone();
        hdr("authorization") == auth
    };

    let segs: Vec<String> = path
        .trim_start_matches('/')
        .splitn(2, '/')
        .map(decode)
        .collect();
    let bucket = segs.first().cloned().unwrap_or_default();
    let key = segs.get(1).cloned().unwrap_or_default();

    let mut st = store.lock().unwrap();
    st.requests.push(format!("{method} {target}"));

    type Reply = (u16, Vec<(String, String)>, Vec<u8>);
    let xml_ct = || vec![("content-type".to_string(), "application/xml".to_string())];
    let error = |status: u16, code: &str, msg: &str| -> Reply {
        (
            status,
            vec![],
            format!("<Error><Code>{code}</Code><Message>{msg}</Message></Error>").into_bytes(),
        )
    };

    let (status, resp_headers, resp_body): Reply = if !ok {
        error(403, "SignatureDoesNotMatch", "bad signature")
    } else if bucket == "forbidden" {
        error(403, "AccessDenied", "Access Denied")
    } else if key.contains("flaky") && st.fail_first > 0 {
        st.fail_first -= 1;
        error(503, "SlowDown", "try later")
    } else if bucket.is_empty() {
        (200, xml_ct(), b"<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>".to_vec())
    } else if method == "POST" && key.is_empty() && q("delete").is_some() {
        // DeleteObjects: Content-MD5 must match
        assert_eq!(
            hdr("content-md5"),
            s3pe::sigv4::base64(&Md5::digest(&body)),
            "Content-MD5"
        );
        let text = String::from_utf8_lossy(&body).to_string();
        let mut out = String::from("<DeleteResult>");
        for k in text.split("<Key>").skip(1) {
            let k = k
                .split("</Key>")
                .next()
                .unwrap()
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">");
            if k == "undeletable" {
                out.push_str("<Error><Key>undeletable</Key><Code>AccessDenied</Code><Message>nope</Message></Error>");
            } else {
                st.objects.remove(&(bucket.clone(), k));
            }
        }
        out.push_str("</DeleteResult>");
        (200, xml_ct(), out.into_bytes())
    } else if method == "GET" && key.is_empty() {
        let prefix = q("prefix").unwrap_or_default();
        let delimiter = q("delimiter");
        let from: usize = q("continuation-token")
            .map(|v| v.parse().unwrap())
            .unwrap_or(0);
        let mut objects: Vec<(&String, &Stored)> = Vec::new();
        let mut prefixes: Vec<String> = Vec::new();
        for ((b, k), o) in st.objects.iter() {
            if *b != bucket || !k.starts_with(&prefix) {
                continue;
            }
            match &delimiter {
                Some(d) if k[prefix.len()..].contains(d.as_str()) => {
                    let rest = &k[prefix.len()..];
                    let cp = format!("{prefix}{}{d}", rest.split(d.as_str()).next().unwrap());
                    if !prefixes.contains(&cp) {
                        prefixes.push(cp);
                    }
                }
                _ => objects.push((k, o)),
            }
        }
        let page: Vec<_> = objects.iter().skip(from).take(PAGE).collect();
        let truncated = from + PAGE < objects.len();
        let mut xml =
            String::from("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        for (k, o) in page.iter() {
            xml.push_str(&format!(
                "<Contents><Key>{}</Key><Size>{}</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>\"{}\"</ETag></Contents>",
                k.replace('&', "&amp;"), o.data.len(), o.etag
            ));
        }
        if from == 0 {
            for p in &prefixes {
                xml.push_str(&format!(
                    "<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>"
                ));
            }
        }
        xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
        if truncated {
            xml.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                from + PAGE
            ));
        }
        xml.push_str("</ListBucketResult>");
        (200, xml_ct(), xml.into_bytes())
    } else if method == "POST" && q("uploads").is_some() {
        st.next_upload += 1;
        let id = format!("upload-{}", st.next_upload);
        st.uploads.insert(id.clone(), BTreeMap::new());
        // remember the content type and metadata of the initiate request on a placeholder
        let meta: BTreeMap<String, String> = headers
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("x-amz-meta-")
                    .map(|m| (m.to_string(), v.clone()))
            })
            .collect();
        st.objects.insert(
            (bucket.clone(), format!("__init__/{id}")),
            Stored {
                data: vec![],
                content_type: hdr("content-type"),
                metadata: meta,
                etag: String::new(),
            },
        );
        (200, xml_ct(), format!("<InitiateMultipartUploadResult><UploadId>{id}</UploadId></InitiateMultipartUploadResult>").into_bytes())
    } else if method == "PUT" && q("partNumber").is_some() {
        let n: usize = q("partNumber").unwrap().parse().unwrap();
        let id = q("uploadId").unwrap();
        if st.fail_part == Some(n) {
            error(500, "InternalError", "part refused")
        } else if let Some(parts) = st.uploads.get_mut(&id) {
            let etag = md5hex(&body);
            parts.insert(n, body);
            (200, vec![("etag".into(), format!("\"{etag}\""))], vec![])
        } else {
            error(404, "NoSuchUpload", "no such upload")
        }
    } else if method == "POST" && q("uploadId").is_some() {
        let id = q("uploadId").unwrap();
        match st.uploads.remove(&id) {
            Some(parts) => {
                let mut data = Vec::new();
                let mut md5s = Vec::new();
                for p in parts.values() {
                    md5s.push(md5hex(p));
                    data.extend_from_slice(p);
                }
                let etag = s3pe::multipart_etag(&md5s);
                let init = st
                    .objects
                    .remove(&(bucket.clone(), format!("__init__/{id}")))
                    .unwrap();
                st.objects.insert(
                    (bucket.clone(), key.clone()),
                    Stored {
                        data,
                        content_type: init.content_type,
                        metadata: init.metadata,
                        etag: etag.clone(),
                    },
                );
                (200, xml_ct(), format!("<CompleteMultipartUploadResult><ETag>\"{etag}\"</ETag></CompleteMultipartUploadResult>").into_bytes())
            }
            None => error(404, "NoSuchUpload", "no such upload"),
        }
    } else if method == "DELETE" && q("uploadId").is_some() {
        let id = q("uploadId").unwrap();
        st.uploads.remove(&id);
        st.objects
            .remove(&(bucket.clone(), format!("__init__/{id}")));
        (204, vec![], vec![])
    } else if method == "PUT" && !hdr("x-amz-copy-source").is_empty() {
        let src = hdr("x-amz-copy-source");
        let segs: Vec<String> = src
            .trim_start_matches('/')
            .splitn(2, '/')
            .map(decode)
            .collect();
        match st.objects.get(&(segs[0].clone(), segs[1].clone())).cloned() {
            Some(o) => {
                let etag = o.etag.clone();
                st.objects.insert((bucket.clone(), key.clone()), o);
                (
                    200,
                    xml_ct(),
                    format!("<CopyObjectResult><ETag>\"{etag}\"</ETag></CopyObjectResult>")
                        .into_bytes(),
                )
            }
            None => error(404, "NoSuchKey", "no source"),
        }
    } else if method == "PUT" {
        let etag = if key == "bad-etag" {
            "00000000000000000000000000000000".to_string()
        } else {
            md5hex(&body)
        };
        let metadata: BTreeMap<String, String> = headers
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("x-amz-meta-")
                    .map(|m| (m.to_string(), v.clone()))
            })
            .collect();
        st.objects.insert(
            (bucket, key),
            Stored {
                data: body,
                content_type: hdr("content-type"),
                metadata,
                etag: etag.clone(),
            },
        );
        (200, vec![("etag".into(), format!("\"{etag}\""))], vec![])
    } else if method == "DELETE" {
        st.objects.remove(&(bucket, key));
        (204, vec![], vec![])
    } else if let Some(o) = st.objects.get(&(bucket.clone(), key.clone())) {
        let mut h = vec![
            ("etag".to_string(), format!("\"{}\"", o.etag)),
            ("content-type".to_string(), o.content_type.clone()),
            (
                "last-modified".to_string(),
                "Thu, 23 Oct 2026 12:00:00 GMT".to_string(),
            ),
        ];
        for (k, v) in &o.metadata {
            h.push((format!("x-amz-meta-{k}"), v.clone()));
        }
        if method == "HEAD" {
            h.push(("content-length".into(), o.data.len().to_string()));
        }
        (
            200,
            h,
            if method == "HEAD" {
                vec![]
            } else {
                o.data.clone()
            },
        )
    } else {
        error(404, "NoSuchKey", "The specified key does not exist.")
    };
    drop(st);

    let mut out = format!("HTTP/1.1 {status} X\r\nconnection: close\r\n");
    if !resp_headers.iter().any(|(k, _)| k == "content-length") {
        out.push_str(&format!("content-length: {}\r\n", resp_body.len()));
    }
    for (k, v) in resp_headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    conn.write_all(out.as_bytes()).unwrap();
    conn.write_all(&resp_body).unwrap();
    conn.flush().unwrap();
}

fn client(fake: &Fake) -> Client {
    Client::builder()
        .endpoint(&fake.url)
        .credentials(ACCESS, SECRET)
        .region("us-east-1")
        .bucket("test")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn tmpdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("s3pe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn round_trip_with_verified_signatures() {
    let fake = start();
    let s3 = client(&fake);

    assert!(!s3.exists("data/manifest.json").unwrap());
    assert!(s3.head("data/manifest.json").unwrap().is_none());
    assert!(matches!(
        s3.read("data/manifest.json"),
        Err(Error::NotFound { .. })
    ));

    let t = s3
        .write("data/manifest.json", br#"{"version":202608}"#, None)
        .unwrap();
    assert_eq!(t.size, 18);
    assert_eq!(t.url(), "s3://test/data/manifest.json");
    assert_eq!(
        s3.read("data/manifest.json").unwrap(),
        br#"{"version":202608}"#
    );
    assert_eq!(s3.read("s3://test/data/manifest.json").unwrap().len(), 18);

    let h = s3.head("data/manifest.json").unwrap().unwrap();
    assert_eq!(h.size, 18);
    assert_eq!(h.content_type.as_deref(), Some("application/json"));
    assert_eq!(h.etag.len(), 32);
    assert!(s3.exists("data/manifest.json").unwrap());

    s3.delete("data/manifest.json").unwrap();
    assert!(!s3.exists("data/manifest.json").unwrap());
    s3.delete("data/manifest.json").unwrap(); // idempotent
}

#[test]
fn keys_with_special_characters_round_trip() {
    let fake = start();
    let s3 = client(&fake);
    let key = "pasta/ação & x+y (1).csv";
    s3.write(key, b"a,b\n1,2\n", None).unwrap();
    assert_eq!(s3.read(key).unwrap(), b"a,b\n1,2\n");
    let listed = s3.list("pasta/").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, key);
    assert_eq!(listed[0].size, 8);
    let stored = fake.store.lock().unwrap();
    assert_eq!(stored.objects.keys().next().unwrap().1, key);
    assert_eq!(
        stored.objects.values().next().unwrap().content_type,
        "text/csv"
    );
}

#[test]
fn listing_follows_continuation_tokens_and_delimiters() {
    let fake = start();
    let s3 = client(&fake);
    for i in 0..5 {
        s3.write(&format!("p/{i}.txt"), format!("{i}"), None)
            .unwrap();
    }
    s3.write("p/sub/a.txt", "a", None).unwrap();
    s3.write("p/sub/deeper/b.txt", "b", None).unwrap();
    s3.write("q/other.txt", "x", None).unwrap();
    let all = s3.list("p/").unwrap();
    assert_eq!(all.len(), 7);
    assert_eq!(s3.list("").unwrap().len(), 8);
    assert_eq!(s3.list("s3://test").unwrap().len(), 8);
    assert_eq!(s3.list("nothing/").unwrap().len(), 0);
    let dir = s3.list_dir("p/").unwrap();
    assert_eq!(dir.objects.len(), 5);
    assert_eq!(dir.prefixes, vec!["p/sub/"]);
    let root = s3.list_dir("").unwrap();
    assert_eq!(root.objects.len(), 0);
    assert_eq!(root.prefixes, vec!["p/", "q/"]);
    assert_eq!(s3.buckets().unwrap()[0].name, "test");
}

#[test]
fn transient_errors_are_retried_and_client_errors_are_not() {
    let fake = start();
    let s3 = client(&fake);
    fake.store.lock().unwrap().fail_first = 1;
    s3.write("flaky.txt", "ok", None).unwrap();
    assert_eq!(s3.read("flaky.txt").unwrap(), b"ok");

    fake.store.lock().unwrap().fail_first = 5;
    let err = s3.read("flaky.txt").unwrap_err();
    assert!(
        matches!(err, Error::Http { status: 503, ref code, .. } if code == "SlowDown"),
        "{err}"
    );

    let before = fake.store.lock().unwrap().requests.len();
    let err = s3.read("s3://forbidden/x").unwrap_err();
    assert!(matches!(err, Error::AccessDenied { .. }), "{err}");
    assert_eq!(
        fake.store.lock().unwrap().requests.len(),
        before + 1,
        "no retry on 403"
    );
}

#[test]
fn upload_integrity_is_checked() {
    let fake = start();
    let s3 = client(&fake);
    let err = s3.write("bad-etag", "data", None).unwrap_err();
    assert!(matches!(err, Error::Integrity(_)), "{err}");
}

#[test]
fn files_go_up_and_down() {
    let fake = start();
    let s3 = client(&fake);
    let dir = tmpdir("files");
    let src = dir.join("sales.parquet");
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &big).unwrap();

    let t = s3.upload(&src, "etl/", None).unwrap();
    assert_eq!(t.key, "etl/sales.parquet");
    assert_eq!(t.size, big.len() as u64);
    assert_eq!(
        fake.store.lock().unwrap().objects[&("test".into(), "etl/sales.parquet".into())]
            .content_type,
        "application/vnd.apache.parquet"
    );

    let dest = dir.join("sub").join("copy.parquet");
    let t = s3.download("etl/sales.parquet", &dest).unwrap();
    assert_eq!(t.size, big.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), big);
    assert!(!dir.join("sub").join("copy.parquet.s3pe-part").exists());

    let err = s3
        .download("etl/missing.parquet", dir.join("missing"))
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
    assert!(!dir.join("missing").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn multipart_upload_from_memory_and_file() {
    let fake = start();
    let s3 = Client::builder()
        .endpoint(&fake.url)
        .credentials(ACCESS, SECRET)
        .bucket("test")
        .multipart_threshold(1024 * 1024)
        .part_size(5 * 1024 * 1024)
        .parallel(3)
        .build()
        .unwrap();
    let big: Vec<u8> = (0..11 * 1024 * 1024 + 7u32)
        .map(|i| (i % 253) as u8)
        .collect();
    let opts = PutOptions {
        content_type: Some("application/x-test".into()),
        metadata: vec![("origin".into(), "test".into())],
        ..Default::default()
    };
    let t = s3.write_with("big/mem.bin", &big, &opts).unwrap();
    assert_eq!(t.size, big.len() as u64);
    assert!(t.etag.ends_with("-3"), "{}", t.etag);
    {
        let st = fake.store.lock().unwrap();
        let o = &st.objects[&("test".into(), "big/mem.bin".into())];
        assert_eq!(o.data, big);
        assert_eq!(o.content_type, "application/x-test");
        assert_eq!(o.metadata["origin"], "test");
        assert!(st.uploads.is_empty());
        assert_eq!(
            st.requests
                .iter()
                .filter(|r| r.contains("partNumber"))
                .count(),
            3
        );
    }
    assert_eq!(s3.read("big/mem.bin").unwrap(), big);
    let h = s3.head("big/mem.bin").unwrap().unwrap();
    assert_eq!(h.metadata["origin"], "test");

    let dir = tmpdir("multipart");
    let f = dir.join("big.bin");
    std::fs::write(&f, &big).unwrap();
    let t = s3.upload(&f, "big/file.bin", None).unwrap();
    assert!(t.etag.ends_with("-3"));
    assert_eq!(
        fake.store.lock().unwrap().objects[&("test".into(), "big/file.bin".into())].data,
        big
    );
    assert_eq!(
        s3pe::expected_etag(&f, 1024 * 1024, 5 * 1024 * 1024).unwrap(),
        t.etag
    );

    // a failing part aborts the upload and leaves nothing behind
    fake.store.lock().unwrap().fail_part = Some(2);
    let s3_once = Client::builder()
        .endpoint(&fake.url)
        .credentials(ACCESS, SECRET)
        .bucket("test")
        .multipart_threshold(1024 * 1024)
        .part_size(5 * 1024 * 1024)
        .max_tries(1)
        .build()
        .unwrap();
    let err = s3_once.write("big/fails.bin", &big, None).unwrap_err();
    assert!(matches!(err, Error::Http { status: 500, .. }), "{err}");
    let st = fake.store.lock().unwrap();
    assert!(st.uploads.is_empty(), "upload aborted");
    assert!(!st
        .objects
        .contains_key(&("test".into(), "big/fails.bin".into())));
    assert!(st
        .requests
        .iter()
        .any(|r| r.starts_with("DELETE") && r.contains("uploadId")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn copy_batch_delete_metadata_and_headers() {
    let fake = start();
    let s3 = client(&fake);
    let opts = PutOptions {
        content_type: None,
        metadata: vec![("Author".into(), "ana".into())],
        headers: vec![
            ("Cache-Control".into(), "max-age=60".into()),
            ("x-amz-storage-class".into(), "STANDARD".into()),
        ],
    };
    s3.write_with("a/one.txt", "1", &opts).unwrap();
    let h = s3.head("a/one.txt").unwrap().unwrap();
    assert_eq!(h.metadata.get("author").map(String::as_str), Some("ana"));

    let t = s3.copy("a/one.txt", "s3://test/b/one-copy.txt").unwrap();
    assert_eq!(t.key, "b/one-copy.txt");
    assert_eq!(s3.read("b/one-copy.txt").unwrap(), b"1");
    assert_eq!(
        s3.head("b/one-copy.txt").unwrap().unwrap().metadata["author"],
        "ana"
    );
    assert!(matches!(
        s3.copy("a/missing", "b/x"),
        Err(Error::NotFound { .. })
    ));

    for i in 0..5 {
        s3.write(&format!("d/{i}.txt"), "x", None).unwrap();
    }
    s3.write("d/a&b.txt", "x", None).unwrap();
    let keys: Vec<String> = s3
        .list("d/")
        .unwrap()
        .into_iter()
        .map(|o| format!("s3://test/{}", o.key))
        .collect();
    assert_eq!(keys.len(), 6);
    s3.delete_many(&keys).unwrap();
    assert_eq!(s3.list("d/").unwrap().len(), 0);
    let n = fake
        .store
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.contains("?delete"))
        .count();
    assert_eq!(n, 1, "one DeleteObjects request");

    s3.write("undeletable", "x", None).unwrap();
    let err = s3.delete_many(&["undeletable", "a/one.txt"]).unwrap_err();
    assert!(
        matches!(err, Error::Http { ref code, .. } if code == "DeleteErrors"),
        "{err}"
    );
    assert!(!s3.exists("a/one.txt").unwrap());
}

#[test]
fn presigned_urls_work_without_credentials() {
    let fake = start();
    let s3 = client(&fake);
    s3.write("pub/report.pdf", b"%PDF", None).unwrap();
    let url = s3
        .presign("pub/report.pdf", "get", Duration::from_secs(600))
        .unwrap();
    assert!(
        url.starts_with(&format!(
            "{}/test/pub/report.pdf?X-Amz-Algorithm=AWS4-HMAC-SHA256&",
            fake.url
        )),
        "{url}"
    );
    assert!(url.contains("X-Amz-Expires=600"));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(&url).call().unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.body_mut().read_to_vec().unwrap(), b"%PDF");

    let put = s3
        .presign("pub/incoming.txt", "PUT", Duration::from_secs(60))
        .unwrap();
    let resp = agent
        .put(&put)
        .header("x-amz-content-sha256", s3pe::sigv4::sha256_hex(b"hello"))
        .send(&b"hello"[..])
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(s3.read("pub/incoming.txt").unwrap(), b"hello");

    let tampered = url.replace("report.pdf", "other.pdf");
    assert_eq!(agent.get(&tampered).call().unwrap().status().as_u16(), 403);
    assert!(matches!(
        s3.presign("k", "DELETE", Duration::from_secs(1)),
        Err(Error::Config(_))
    ));
}

#[test]
fn sync_up_and_down_by_etag() {
    let fake = start();
    let s3 = client(&fake);
    let dir = tmpdir("sync-up");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.csv"), "a").unwrap();
    std::fs::write(dir.join("sub/b.csv"), "b").unwrap();
    std::fs::write(dir.join("manifest.json"), "{}").unwrap();

    let opts = SyncOptions {
        delete: true,
        last: Some("manifest.json".into()),
    };
    let r = s3.sync_up(&dir, "s3://test/out", &opts).unwrap();
    assert_eq!(
        r.transferred,
        vec!["a.csv", "sub/b.csv", "manifest.json"],
        "manifest last"
    );
    assert!(r.skipped.is_empty() && r.deleted.is_empty());
    assert_eq!(s3.list("out/").unwrap().len(), 3);

    // nothing changed: nothing uploaded; one changed file and one stale remote object
    let r = s3.sync_up(&dir, "out/", &opts).unwrap();
    assert!(r.transferred.is_empty());
    assert_eq!(r.skipped.len(), 3);
    std::fs::write(dir.join("a.csv"), "a2").unwrap();
    s3.write("out/stale.txt", "old", None).unwrap();
    let r = s3.sync_up(&dir, "out/", &opts).unwrap();
    assert_eq!(r.transferred, vec!["a.csv"]);
    assert_eq!(r.deleted, vec!["stale.txt"]);
    assert!(!s3.exists("out/stale.txt").unwrap());
    assert!(matches!(
        s3.sync_up(
            &dir,
            "out/",
            &SyncOptions {
                delete: false,
                last: Some("nope".into())
            }
        ),
        Err(Error::Config(_))
    ));

    let down = tmpdir("sync-down");
    std::fs::write(down.join("extra.txt"), "x").unwrap();
    let r = s3
        .sync_down(
            "out/",
            &down,
            &SyncOptions {
                delete: true,
                last: None,
            },
        )
        .unwrap();
    assert_eq!(r.transferred.len(), 3);
    assert_eq!(r.deleted, vec!["extra.txt"]);
    assert_eq!(
        std::fs::read_to_string(down.join("sub/b.csv")).unwrap(),
        "b"
    );
    assert_eq!(std::fs::read_to_string(down.join("a.csv")).unwrap(), "a2");
    let r = s3
        .sync_down("out/", &down, &SyncOptions::default())
        .unwrap();
    assert!(r.transferred.is_empty());
    assert_eq!(r.skipped.len(), 3);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&down).unwrap();
}

#[test]
fn errors_without_credentials_or_bucket() {
    let s3 = Client::builder()
        .endpoint("http://127.0.0.1:1")
        .credentials("a", "b")
        .build()
        .unwrap();
    assert!(matches!(s3.read("x"), Err(Error::Config(_))));
    assert!(matches!(s3.read("s3://b/"), Err(Error::Config(_))));
}
