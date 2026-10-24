package s3pe

import (
	"strings"
	"testing"
	"time"
)

// The example of the AWS documentation ("GET Object", signature v4, header
// authorisation). Every edition of s3pe reproduces it (DESIGN.md, D8).
func TestAWSDocumentationExample(t *testing.T) {
	creds := credentials{
		accessKey: "AKIAIOSFODNN7EXAMPLE",
		secretKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
	}
	headers := sign(signRequest{
		method:       "GET",
		host:         "examplebucket.s3.amazonaws.com",
		path:         "/test.txt",
		payloadHash:  emptySHA256,
		amzDate:      "20130524T000000Z",
		extraHeaders: []pair{{"range", "bytes=0-9"}},
	}, creds, "us-east-1")
	var auth string
	for _, h := range headers {
		if h.name == "authorization" {
			auth = h.value
		}
	}
	want := "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, " +
		"SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, " +
		"Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
	if auth != want {
		t.Fatalf("authorization\n got %s\nwant %s", auth, want)
	}
}

// The pre-signed URL example of the AWS documentation (DESIGN.md, D16).
func TestAWSPresignedExample(t *testing.T) {
	creds := credentials{
		accessKey: "AKIAIOSFODNN7EXAMPLE",
		secretKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
	}
	qs := presign(presignRequest{
		method:  "GET",
		host:    "examplebucket.s3.amazonaws.com",
		path:    "/test.txt",
		amzDate: "20130524T000000Z",
		expires: 86400,
	}, creds, "us-east-1")
	wantPrefix := "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host"
	if !strings.HasPrefix(qs, wantPrefix) {
		t.Fatalf("query\n got %s\nwant prefix %s", qs, wantPrefix)
	}
	if !strings.HasSuffix(qs, "&X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404") {
		t.Fatalf("signature: %s", qs)
	}
}

func TestURIEncode(t *testing.T) {
	cases := []struct {
		in        string
		keepSlash bool
		want      string
	}{
		{"a b/ç~x.y_z-1", true, "a%20b/%C3%A7~x.y_z-1"},
		{"a/b", false, "a%2Fb"},
		{"*+", false, "%2A%2B"},
		{"", true, ""},
	}
	for _, c := range cases {
		if got := uriEncode(c.in, c.keepSlash); got != c.want {
			t.Errorf("uriEncode(%q, %v) = %q, want %q", c.in, c.keepSlash, got, c.want)
		}
	}
}

func TestCanonicalQuery(t *testing.T) {
	got := canonicalQuery([]pair{{"prefix", "dados/x y"}, {"list-type", "2"}})
	if want := "list-type=2&prefix=dados%2Fx%20y"; got != want {
		t.Errorf("got %q, want %q", got, want)
	}
	if got := canonicalQuery(nil); got != "" {
		t.Errorf("empty query gave %q", got)
	}
}

func TestAmzDate(t *testing.T) {
	cases := map[int64]string{
		0:          "19700101T000000Z",
		1369353600: "20130524T000000Z",
		1761264000: "20251024T000000Z",
		951782400:  "20000229T000000Z",
	}
	for unix, want := range cases {
		if got := amzDate(time.Unix(unix, 0)); got != want {
			t.Errorf("amzDate(%d) = %s, want %s", unix, got, want)
		}
	}
}
