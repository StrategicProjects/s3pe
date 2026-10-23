package s3pe

// AWS Signature Version 4 for S3, header style. Signed headers are host,
// x-amz-content-sha256, x-amz-date and, with a session token,
// x-amz-security-token. The payload hash is always the real SHA-256 of the
// body: never UNSIGNED-PAYLOAD, never aws-chunked (Garage 2.4 rejects the
// chunked trailer).

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"sort"
	"strings"
	"time"
)

// emptySHA256 is the payload hash of GET, HEAD and DELETE.
const emptySHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

func sha256Hex(b []byte) string {
	s := sha256.Sum256(b)
	return hex.EncodeToString(s[:])
}

func hmacSHA256(key, msg []byte) []byte {
	m := hmac.New(sha256.New, key)
	m.Write(msg)
	return m.Sum(nil)
}

// uriEncode percent-encodes the way AWS canonicalises: unreserved
// characters (A-Z a-z 0-9 - _ . ~) are kept, everything else becomes %XX
// with upper-case hex. "/" is kept only when keepSlash is true (object keys
// in the path); in query strings it is encoded.
func uriEncode(s string, keepSlash bool) string {
	const hexDigits = "0123456789ABCDEF"
	var b strings.Builder
	b.Grow(len(s) + 8)
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= 'A' && c <= 'Z', c >= 'a' && c <= 'z', c >= '0' && c <= '9',
			c == '-', c == '_', c == '.', c == '~':
			b.WriteByte(c)
		case c == '/' && keepSlash:
			b.WriteByte(c)
		default:
			b.WriteByte('%')
			b.WriteByte(hexDigits[c>>4])
			b.WriteByte(hexDigits[c&15])
		}
	}
	return b.String()
}

// pair is a name/value couple: a query parameter or a header.
type pair struct{ name, value string }

// canonicalQuery encodes the parameters and sorts them by name, then value.
func canonicalQuery(params []pair) string {
	enc := make([]pair, len(params))
	for i, p := range params {
		enc[i] = pair{uriEncode(p.name, false), uriEncode(p.value, false)}
	}
	sort.Slice(enc, func(i, j int) bool {
		if enc[i].name != enc[j].name {
			return enc[i].name < enc[j].name
		}
		return enc[i].value < enc[j].value
	})
	parts := make([]string, len(enc))
	for i, p := range enc {
		parts[i] = p.name + "=" + p.value
	}
	return strings.Join(parts, "&")
}

type credentials struct {
	accessKey, secretKey, sessionToken string
}

// signRequest is what the signer needs to know about a request.
type signRequest struct {
	method       string // upper case
	host         string // value of the Host header (host[:port])
	path         string // canonical path, already encoded, starting with "/"
	query        []pair // raw (unencoded) parameters
	payloadHash  string // hex SHA-256 of the payload
	amzDate      string // YYYYMMDDTHHMMSSZ
	extraHeaders []pair // more headers to sign, lower-case names (rare)
}

// sign returns the headers to add to the request: x-amz-date,
// x-amz-content-sha256, x-amz-security-token (when present), the extra
// headers and authorization. Host is left to the HTTP client.
func sign(req signRequest, creds credentials, region string) []pair {
	headers := []pair{
		{"host", req.host},
		{"x-amz-content-sha256", req.payloadHash},
		{"x-amz-date", req.amzDate},
	}
	if creds.sessionToken != "" {
		headers = append(headers, pair{"x-amz-security-token", creds.sessionToken})
	}
	for _, h := range req.extraHeaders {
		headers = append(headers, pair{strings.ToLower(h.name), strings.TrimSpace(h.value)})
	}
	sort.Slice(headers, func(i, j int) bool {
		if headers[i].name != headers[j].name {
			return headers[i].name < headers[j].name
		}
		return headers[i].value < headers[j].value
	})

	var canonicalHeaders strings.Builder
	names := make([]string, len(headers))
	for i, h := range headers {
		canonicalHeaders.WriteString(h.name + ":" + h.value + "\n")
		names[i] = h.name
	}
	signedHeaders := strings.Join(names, ";")

	canonicalRequest := strings.Join([]string{
		req.method,
		req.path,
		canonicalQuery(req.query),
		canonicalHeaders.String(),
		signedHeaders,
		req.payloadHash,
	}, "\n")

	date := req.amzDate[:8]
	scope := date + "/" + region + "/s3/aws4_request"
	stringToSign := strings.Join([]string{
		"AWS4-HMAC-SHA256",
		req.amzDate,
		scope,
		sha256Hex([]byte(canonicalRequest)),
	}, "\n")

	kDate := hmacSHA256([]byte("AWS4"+creds.secretKey), []byte(date))
	kRegion := hmacSHA256(kDate, []byte(region))
	kService := hmacSHA256(kRegion, []byte("s3"))
	kSigning := hmacSHA256(kService, []byte("aws4_request"))
	signature := hex.EncodeToString(hmacSHA256(kSigning, []byte(stringToSign)))

	authorization := fmt.Sprintf(
		"AWS4-HMAC-SHA256 Credential=%s/%s, SignedHeaders=%s, Signature=%s",
		creds.accessKey, scope, signedHeaders, signature)

	out := make([]pair, 0, len(headers))
	for _, h := range headers {
		if h.name != "host" {
			out = append(out, h)
		}
	}
	return append(out, pair{"authorization", authorization})
}

// amzDate formats a time as YYYYMMDDTHHMMSSZ (UTC).
func amzDate(t time.Time) string {
	return t.UTC().Format("20060102T150405Z")
}
