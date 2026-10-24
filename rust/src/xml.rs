//! The XML documents S3 answers with: listings, bucket lists, errors,
//! multipart and copy results, batch-delete results.

use crate::client::{Bucket, Object};

fn text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> String {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .to_string()
}

/// One page of a `ListObjectsV2` answer.
pub struct ListingPage {
    pub objects: Vec<Object>,
    pub prefixes: Vec<String>,
    pub next: Option<String>,
}

/// Objects, common prefixes, `IsTruncated` and the continuation token of a
/// `ListObjectsV2`.
pub fn parse_listing(body: &str) -> Result<ListingPage, String> {
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
            metadata: Default::default(),
        })
        .collect();
    let prefixes = root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "CommonPrefixes")
        .map(|c| text(c, "Prefix"))
        .filter(|p| !p.is_empty())
        .collect();
    let truncated = text(root, "IsTruncated") == "true";
    let token = text(root, "NextContinuationToken");
    let next = if truncated && !token.is_empty() {
        Some(token)
    } else {
        None
    };
    Ok(ListingPage {
        objects,
        prefixes,
        next,
    })
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
            if root.tag_name().name() == "Error" {
                (text(root, "Code"), text(root, "Message"))
            } else {
                (String::new(), String::new())
            }
        }
        Err(_) => (String::new(), String::new()),
    }
}

/// The text of the first element called `name`, anywhere in the document,
/// without surrounding quotes.
pub fn first_text(body: &str, name: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(body).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(|t| t.trim_matches('"').to_string())
}

/// `(Key, Code, Message)` of every `<Error>` in a `DeleteResult`.
pub fn parse_delete_errors(body: &str) -> Vec<(String, String, String)> {
    match roxmltree::Document::parse(body) {
        Ok(doc) => doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Error")
            .map(|n| (text(n, "Key"), text(n, "Code"), text(n, "Message")))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Escapes text for an XML element.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
        let page = parse_listing(xml).unwrap();
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].key, "raw/manifest.json");
        assert_eq!(page.objects[0].size, 1234);
        assert_eq!(page.objects[0].etag, "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(page.next.as_deref(), Some("abc"));
        assert!(page.prefixes.is_empty());
        let with_prefixes = r#"<ListBucketResult><IsTruncated>false</IsTruncated>
<CommonPrefixes><Prefix>raw/2025/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>raw/2026/</Prefix></CommonPrefixes></ListBucketResult>"#;
        let page = parse_listing(with_prefixes).unwrap();
        assert_eq!(page.prefixes, vec!["raw/2025/", "raw/2026/"]);
    }

    #[test]
    fn buckets_errors_and_fragments() {
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
        assert_eq!(
            parse_error("<CopyObjectResult><ETag>x</ETag></CopyObjectResult>"),
            ("".into(), "".into())
        );

        assert_eq!(first_text("<InitiateMultipartUploadResult><UploadId>abc-1</UploadId></InitiateMultipartUploadResult>", "UploadId").as_deref(), Some("abc-1"));
        assert_eq!(
            first_text(
                "<CopyObjectResult><ETag>\"9e10\"</ETag></CopyObjectResult>",
                "ETag"
            )
            .as_deref(),
            Some("9e10")
        );
        let errs = parse_delete_errors("<DeleteResult><Deleted><Key>a</Key></Deleted><Error><Key>b</Key><Code>AccessDenied</Code><Message>no</Message></Error></DeleteResult>");
        assert_eq!(
            errs,
            vec![(
                "b".to_string(),
                "AccessDenied".to_string(),
                "no".to_string()
            )]
        );
        assert_eq!(escape("a&b<c>"), "a&amp;b&lt;c&gt;");
    }
}
