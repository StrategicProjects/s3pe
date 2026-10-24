//! Against a real store, only when `S3PE_ENDPOINT` is set and
//! `S3PE_TEST_BUCKET` names a bucket the credentials in the environment can
//! write to. Everything goes under `s3pe-test/<pid>/` and is deleted at the end.

use s3pe::{Client, PutOptions, SyncOptions};
use std::time::Duration;

struct Stderr;
impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        eprintln!("[{}] {}", r.level(), r.args());
    }
    fn flush(&self) {}
}
static LOGGER: Stderr = Stderr;

#[test]
fn live_round_trip() {
    let (Ok(bucket), Ok(_)) = (
        std::env::var("S3PE_TEST_BUCKET"),
        std::env::var("S3PE_ENDPOINT"),
    ) else {
        eprintln!("S3PE_TEST_BUCKET / S3PE_ENDPOINT not set: live test skipped");
        return;
    };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Debug);
    let s3 = Client::builder()
        .bucket(&bucket)
        .multipart_threshold(6 * 1024 * 1024)
        .part_size(5 * 1024 * 1024)
        .build()
        .unwrap();
    let prefix = format!("s3pe-test/{}/", std::process::id());
    let key = format!("{prefix}ação e espaço.csv");

    // plain objects, metadata
    assert!(!s3.exists(&key).unwrap());
    let opts = PutOptions {
        metadata: vec![("origin".into(), "live-test".into())],
        ..Default::default()
    };
    let t = s3.write_with(&key, "a,b\n1,2\n", &opts).unwrap();
    assert_eq!(t.size, 8);
    assert_eq!(s3.read(&key).unwrap(), b"a,b\n1,2\n");
    let h = s3.head(&key).unwrap().unwrap();
    assert_eq!(h.size, 8);
    assert_eq!(h.content_type.as_deref(), Some("text/csv"));
    assert_eq!(
        h.metadata.get("origin").map(String::as_str),
        Some("live-test")
    );

    // multipart (12 MiB, three parts of 5 MiB)
    let big: Vec<u8> = (0..12 * 1024 * 1024u32).map(|i| (i % 253) as u8).collect();
    let dir = std::env::temp_dir().join(format!("s3pe-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("big.bin"), &big).unwrap();
    let t = s3
        .upload(dir.join("big.bin"), &format!("{prefix}big.bin"), None)
        .unwrap();
    assert!(t.etag.ends_with("-3"), "multipart etag: {}", t.etag);
    s3.download(&format!("{prefix}big.bin"), dir.join("back.bin"))
        .unwrap();
    assert_eq!(std::fs::read(dir.join("back.bin")).unwrap(), big);

    // copy, one-level listing
    s3.copy(&key, &format!("{prefix}sub/copy.csv")).unwrap();
    assert_eq!(
        s3.read(&format!("{prefix}sub/copy.csv")).unwrap(),
        b"a,b\n1,2\n"
    );
    let dir_listing = s3.list_dir(&prefix).unwrap();
    assert_eq!(dir_listing.objects.len(), 2);
    assert_eq!(dir_listing.prefixes, vec![format!("{prefix}sub/")]);

    // pre-signed GET without credentials
    let url = s3.presign(&key, "GET", Duration::from_secs(300)).unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(&url).call().unwrap();
    assert_eq!(resp.status().as_u16(), 200, "presigned GET");
    assert_eq!(resp.body_mut().read_to_vec().unwrap(), b"a,b\n1,2\n");

    // sync up / down
    let local = dir.join("sync");
    std::fs::create_dir_all(local.join("sub")).unwrap();
    std::fs::write(local.join("a.txt"), "a").unwrap();
    std::fs::write(local.join("sub/b.txt"), "b").unwrap();
    std::fs::write(local.join("manifest.json"), "{}").unwrap();
    let r = s3
        .sync_up(
            &local,
            &format!("{prefix}sync/"),
            &SyncOptions {
                delete: false,
                last: Some("manifest.json".into()),
            },
        )
        .unwrap();
    assert_eq!(r.transferred.len(), 3);
    assert_eq!(
        r.transferred.last().map(String::as_str),
        Some("manifest.json")
    );
    let r = s3
        .sync_up(&local, &format!("{prefix}sync/"), &SyncOptions::default())
        .unwrap();
    assert_eq!(r.skipped.len(), 3);
    let back = dir.join("sync-back");
    let r = s3
        .sync_down(&format!("{prefix}sync/"), &back, &SyncOptions::default())
        .unwrap();
    assert_eq!(r.transferred.len(), 3);
    assert_eq!(
        std::fs::read_to_string(back.join("sub/b.txt")).unwrap(),
        "b"
    );

    // batch delete everything
    let all: Vec<String> = s3
        .list(&prefix)
        .unwrap()
        .into_iter()
        .map(|o| o.key)
        .collect();
    assert_eq!(all.len(), 6, "{all:?}");
    s3.delete_many(&all).unwrap();
    assert_eq!(s3.list(&prefix).unwrap().len(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
