//! The three XML documents S3 answers with: listings, bucket lists, errors.

use crate::client::{Bucket, Object};

fn text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> String {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .to_string()
}

/// Objects, `IsTruncated` and the continuation token of a `ListObjectsV2`.
pub fn parse_listing(body: &str) -> Result<(Vec<Object>, Option<String>), String> {
    let doc = roxmltree::Document::parse(body).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let objects = root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "Contents")
        .map(|c| Object {
            key: text(c, "Key"),
            size: text(c, "Size").parse().unwrap_or(0),
            last_modified: text(c, "LastModified"),
            etag: text(c, "ETag").trim_matches('"').to_string(),
            content_type: None,
        })
        .collect();
    let truncated = text(root, "IsTruncated") == "true";
    let token = text(root, "NextContinuationToken");
    let next = if truncated && !token.is_empty() {
        Some(token)
    } else {
        None
    };
    Ok((objects, next))
}

/// Buckets of a `ListBuckets`.
pub fn parse_buckets(body: &str) -> Result<Vec<Bucket>, String> {
    let doc = roxmltree::Document::parse(body).map_err(|e| e.to_string())?;
    Ok(doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Bucket")
        .map(|n| Bucket {
            name: text(n, "Name"),
            created: text(n, "CreationDate"),
        })
        .collect())
}

/// `(Code, Message)` of an S3 error document; empty strings when the body is
/// not one (nginx and proxies answer with HTML).
pub fn parse_error(body: &str) -> (String, String) {
    match roxmltree::Document::parse(body) {
        Ok(doc) => {
            let root = doc.root_element();
            (text(root, "Code"), text(root, "Message"))
        }
        Err(_) => (String::new(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_with_namespace_and_token() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>data</Name><Prefix>raw/</Prefix><KeyCount>2</KeyCount>
  <IsTruncated>true</IsTruncated><NextContinuationToken>abc</NextContinuationToken>
  <Contents><Key>raw/manifest.json</Key><LastModified>2026-10-20T04:31:02.000Z</LastModified>
    <ETag>"9e107d9d372bb6826bd81d3542a419d6"</ETag><Size>1234</Size></Contents>
  <Contents><Key>raw/series.json</Key><LastModified>2026-10-20T04:31:03.000Z</LastModified>
    <ETag>"x"</ETag><Size>99</Size></Contents>
</ListBucketResult>"#;
        let (objects, next) = parse_listing(xml).unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].key, "raw/manifest.json");
        assert_eq!(objects[0].size, 1234);
        assert_eq!(objects[0].etag, "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(next.as_deref(), Some("abc"));
    }

    #[test]
    fn buckets_and_errors() {
        let xml = r#"<ListAllMyBucketsResult><Owner><ID>me</ID></Owner><Buckets>
<Bucket><Name>data</Name><CreationDate>2026-10-20T00:00:00Z</CreationDate></Bucket>
<Bucket><Name>reports</Name><CreationDate>2026-10-21T00:00:00Z</CreationDate></Bucket>
</Buckets></ListAllMyBucketsResult>"#;
        let b = parse_buckets(xml).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].name, "reports");

        let err = r#"<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#;
        assert_eq!(
            parse_error(err),
            (
                "NoSuchKey".into(),
                "The specified key does not exist.".into()
            )
        );
        assert_eq!(parse_error("<html>502</html>"), ("".into(), "".into()));
    }
}
