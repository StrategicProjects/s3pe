//! Against a real store, only when `S3PE_TEST_BUCKET` names a bucket the
//! credentials in the environment can write to (and `S3PE_ENDPOINT` is set). Everything goes under the
//! prefix `s3pe-test/<pid>/` and is deleted at the end.

use s3pe::Client;

#[test]
fn live_round_trip() {
    let Ok(bucket) = std::env::var("S3PE_TEST_BUCKET") else {
        eprintln!("S3PE_TEST_BUCKET not set: live test skipped");
        return;
    };
    let s3 = Client::builder().bucket(bucket).build().unwrap();
    let prefix = format!("s3pe-test/{}/", std::process::id());
    let key = format!("{prefix}ação e espaço.csv");

    assert!(!s3.exists(&key).unwrap());
    let t = s3.write(&key, "a,b\n1,2\n", None).unwrap();
    assert_eq!(t.size, 8);
    assert_eq!(s3.read(&key).unwrap(), b"a,b\n1,2\n");
    let h = s3.head(&key).unwrap().unwrap();
    assert_eq!(h.size, 8);
    assert_eq!(h.content_type.as_deref(), Some("text/csv"));

    let big: Vec<u8> = (0..2_500_000u32).map(|i| (i % 253) as u8).collect();
    let dir = std::env::temp_dir().join(format!("s3pe-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("big.bin"), &big).unwrap();
    s3.upload(dir.join("big.bin"), &format!("{prefix}big.bin"), None)
        .unwrap();
    s3.download(&format!("{prefix}big.bin"), dir.join("back.bin"))
        .unwrap();
    assert_eq!(std::fs::read(dir.join("back.bin")).unwrap(), big);

    let listed = s3.list(&prefix).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|o| o.key == key));

    for o in listed {
        s3.delete(&o.key).unwrap();
    }
    assert_eq!(s3.list(&prefix).unwrap().len(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
