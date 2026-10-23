//! The client against a fake S3 server on localhost that checks every
//! signature with the crate's own signer, stores objects in memory and can
//! misbehave on demand (503s, 403s, wrong ETags).

use md5::{Digest, Md5};
use s3pe::sigv4::{sign, Credentials, SignRequest};
use s3pe::{Client, Error};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ACCESS: &str = "GKtest";
const SECRET: &str = "secret-of-the-fake";
const PAGE: usize = 2;

#[derive(Default)]
struct Store {
    objects: BTreeMap<(String, String), (Vec<u8>, String)>,
    requests: Vec<String>,
    fail_first: u32,
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

    // Signature check: the request must carry exactly what our signer produces.
    let payload_hash = hdr("x-amz-content-sha256");
    assert_eq!(payload_hash, s3pe::sigv4::sha256_hex(&body), "payload hash");
    assert_eq!(hdr("host"), host, "host header");
    let expected = sign(
        &SignRequest {
            method: &method,
            host,
            path,
            query: &query,
            payload_hash: &payload_hash,
            amz_date: &hdr("x-amz-date"),
            extra_headers: &[],
        },
        &Credentials {
            access_key: ACCESS.into(),
            secret_key: SECRET.into(),
            session_token: None,
        },
        "us-east-1",
    );
    let auth = expected
        .iter()
        .find(|(k, _)| k == "authorization")
        .unwrap()
        .1
        .clone();
    let ok = hdr("authorization") == auth;

    let segs: Vec<String> = path
        .trim_start_matches('/')
        .splitn(2, '/')
        .map(decode)
        .collect();
    let bucket = segs.first().cloned().unwrap_or_default();
    let key = segs.get(1).cloned().unwrap_or_default();

    let mut st = store.lock().unwrap();
    st.requests.push(format!("{method} {target}"));

    let (status, resp_headers, resp_body): (u16, Vec<(String, String)>, Vec<u8>) = if !ok {
        (
            403,
            vec![],
            b"<Error><Code>SignatureDoesNotMatch</Code><Message>bad signature</Message></Error>"
                .to_vec(),
        )
    } else if bucket == "forbidden" {
        (
            403,
            vec![],
            b"<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>".to_vec(),
        )
    } else if key.contains("flaky") && st.fail_first > 0 {
        st.fail_first -= 1;
        (
            503,
            vec![],
            b"<Error><Code>SlowDown</Code><Message>try later</Message></Error>".to_vec(),
        )
    } else if bucket.is_empty() {
        (200, vec![("content-type".into(), "application/xml".into())],
         b"<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>".to_vec())
    } else if method == "GET" && key.is_empty() {
        let prefix = query
            .iter()
            .find(|(k, _)| k == "prefix")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let from: usize = query
            .iter()
            .find(|(k, _)| k == "continuation-token")
            .map(|(_, v)| v.parse().unwrap())
            .unwrap_or(0);
        let all: Vec<_> = st
            .objects
            .iter()
            .filter(|((b, k), _)| *b == bucket && k.starts_with(&prefix))
            .collect();
        let page: Vec<_> = all.iter().skip(from).take(PAGE).collect();
        let truncated = from + PAGE < all.len();
        let mut xml =
            String::from("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        for ((_, k), (data, _)) in page.iter() {
            xml.push_str(&format!(
                "<Contents><Key>{}</Key><Size>{}</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>\"{}\"</ETag></Contents>",
                k.replace('&', "&amp;"), data.len(), hex::encode(Md5::digest(data))
            ));
        }
        xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
        if truncated {
            xml.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                from + PAGE
            ));
        }
        xml.push_str("</ListBucketResult>");
        (
            200,
            vec![("content-type".into(), "application/xml".into())],
            xml.into_bytes(),
        )
    } else if method == "PUT" {
        let ct = hdr("content-type");
        let etag = if key == "bad-etag" {
            "00000000000000000000000000000000".to_string()
        } else {
            hex::encode(Md5::digest(&body))
        };
        st.objects.insert((bucket, key), (body, ct));
        (200, vec![("etag".into(), format!("\"{etag}\""))], vec![])
    } else if method == "DELETE" {
        st.objects.remove(&(bucket, key));
        (204, vec![], vec![])
    } else if let Some((data, ct)) = st.objects.get(&(bucket.clone(), key.clone())) {
        let mut h = vec![
            (
                "etag".into(),
                format!("\"{}\"", hex::encode(Md5::digest(data))),
            ),
            ("content-type".into(), ct.clone()),
            (
                "last-modified".into(),
                "Thu, 23 Oct 2026 12:00:00 GMT".into(),
            ),
        ];
        if method == "HEAD" {
            h.push(("content-length".into(), data.len().to_string()));
        }
        (
            200,
            h,
            if method == "HEAD" {
                vec![]
            } else {
                data.clone()
            },
        )
    } else {
        (404, vec![], b"<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>".to_vec())
    };
    drop(st);

    let mut out = format!("HTTP/1.1 {status} X\r\nconnection: close\r\n");
    let has_len = resp_headers.iter().any(|(k, _)| k == "content-length");
    if !has_len {
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

#[test]
fn round_trip_with_verified_signatures() {
    let fake = start();
    let s3 = client(&fake);

    assert!(!s3.exists("dados/metadata.json").unwrap());
    assert!(s3.head("dados/metadata.json").unwrap().is_none());
    assert!(matches!(
        s3.read("dados/metadata.json"),
        Err(Error::NotFound { .. })
    ));

    let t = s3
        .write("dados/metadata.json", br#"{"ultimo_mes":202608}"#, None)
        .unwrap();
    assert_eq!(t.size, 21);
    assert_eq!(t.url(), "s3://test/dados/metadata.json");
    assert_eq!(
        s3.read("dados/metadata.json").unwrap(),
        br#"{"ultimo_mes":202608}"#
    );
    assert_eq!(s3.read("s3://test/dados/metadata.json").unwrap().len(), 21);

    let h = s3.head("dados/metadata.json").unwrap().unwrap();
    assert_eq!(h.size, 21);
    assert_eq!(h.content_type.as_deref(), Some("application/json"));
    assert_eq!(h.etag.len(), 32);
    assert!(s3.exists("dados/metadata.json").unwrap());

    s3.delete("dados/metadata.json").unwrap();
    assert!(!s3.exists("dados/metadata.json").unwrap());
    s3.delete("dados/metadata.json").unwrap(); // idempotent
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
    assert_eq!(stored.objects.values().next().unwrap().1, "text/csv");
}

#[test]
fn listing_follows_continuation_tokens() {
    let fake = start();
    let s3 = client(&fake);
    for i in 0..5 {
        s3.write(&format!("p/{i}.txt"), format!("{i}"), None)
            .unwrap();
    }
    s3.write("q/other.txt", "x", None).unwrap();
    let all = s3.list("p/").unwrap();
    assert_eq!(
        all.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
        ["p/0.txt", "p/1.txt", "p/2.txt", "p/3.txt", "p/4.txt"]
    );
    assert_eq!(s3.list("").unwrap().len(), 6);
    assert_eq!(s3.list("s3://test").unwrap().len(), 6);
    assert_eq!(s3.list("nothing/").unwrap().len(), 0);
    let buckets = s3.buckets().unwrap();
    assert_eq!(buckets[0].name, "test");
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
    let dir = std::env::temp_dir().join(format!("s3pe-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("painel.parquet");
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &big).unwrap();

    let t = s3.upload(&src, "etl/", None).unwrap();
    assert_eq!(t.key, "etl/painel.parquet");
    assert_eq!(t.size, big.len() as u64);
    assert_eq!(
        fake.store.lock().unwrap().objects[&("test".into(), "etl/painel.parquet".into())].1,
        "application/vnd.apache.parquet"
    );

    let dest = dir.join("sub").join("copy.parquet");
    let t = s3.download("etl/painel.parquet", &dest).unwrap();
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
fn errors_without_credentials_or_bucket() {
    let err = Client::builder()
        .endpoint("http://127.0.0.1:1")
        .build()
        .err();
    // The environment of the test machine may carry credentials; only the
    // message of a configuration error is asserted when it happens.
    if let Some(e) = err {
        assert!(matches!(e, Error::Config(_)));
    }
    let s3 = Client::builder()
        .endpoint("http://127.0.0.1:1")
        .credentials("a", "b")
        .build()
        .unwrap();
    assert!(matches!(s3.read("x"), Err(Error::Config(_))));
    assert!(matches!(s3.read("s3://b/"), Err(Error::Config(_))));
}
