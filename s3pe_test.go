package s3pe

import (
	"bytes"
	"context"
	"crypto/md5"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// A fake S3 on localhost: checks every signature with the package's own
// signer, keeps objects in memory and misbehaves on demand.

const (
	fakeAccess = "GKtest"
	fakeSecret = "secret-of-the-fake"
	fakePage   = 2
)

type fakeS3 struct {
	mu        sync.Mutex
	objects   map[string]map[string]fakeObject // bucket -> key -> object
	requests  []string
	failFirst int
}

type fakeObject struct {
	data        []byte
	contentType string
}

func newFake(t *testing.T) (*fakeS3, *httptest.Server) {
	f := &fakeS3{objects: map[string]map[string]fakeObject{}}
	srv := httptest.NewServer(f)
	t.Cleanup(srv.Close)
	return f, srv
}

func (f *fakeS3) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)

	// Signature check: the request must carry what our signer produces.
	if got := r.Header.Get("x-amz-content-sha256"); got != sha256Hex(body) {
		http.Error(w, "payload hash "+got, http.StatusBadRequest)
		return
	}
	var query []pair
	for k, vs := range r.URL.Query() {
		for _, v := range vs {
			query = append(query, pair{k, v})
		}
	}
	want := ""
	for _, h := range sign(signRequest{
		method:      r.Method,
		host:        r.Host,
		path:        r.URL.EscapedPath(),
		query:       query,
		payloadHash: r.Header.Get("x-amz-content-sha256"),
		amzDate:     r.Header.Get("x-amz-date"),
	}, credentials{accessKey: fakeAccess, secretKey: fakeSecret}, "us-east-1") {
		if h.name == "authorization" {
			want = h.value
		}
	}
	signed := r.Header.Get("Authorization") == want

	segs := strings.SplitN(strings.TrimPrefix(r.URL.EscapedPath(), "/"), "/", 2)
	bucket, _ := url.PathUnescape(segs[0])
	key := ""
	if len(segs) == 2 {
		key, _ = url.PathUnescape(segs[1])
	}

	f.mu.Lock()
	defer f.mu.Unlock()
	f.requests = append(f.requests, r.Method+" "+r.URL.RequestURI())

	xmlErr := func(status int, code, msg string) {
		w.Header().Set("Content-Type", "application/xml")
		w.WriteHeader(status)
		fmt.Fprintf(w, "<Error><Code>%s</Code><Message>%s</Message></Error>", code, msg)
	}
	switch {
	case !signed:
		xmlErr(403, "SignatureDoesNotMatch", "bad signature")
	case bucket == "forbidden":
		xmlErr(403, "AccessDenied", "Access Denied")
	case strings.Contains(key, "flaky") && f.failFirst > 0:
		f.failFirst--
		xmlErr(503, "SlowDown", "try later")
	case bucket == "":
		w.Header().Set("Content-Type", "application/xml")
		io.WriteString(w, `<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>`)
	case r.Method == http.MethodGet && key == "":
		prefix := r.URL.Query().Get("prefix")
		from, _ := strconv.Atoi(r.URL.Query().Get("continuation-token"))
		var keys []string
		for k := range f.objects[bucket] {
			if strings.HasPrefix(k, prefix) {
				keys = append(keys, k)
			}
		}
		sort.Strings(keys)
		end := from + fakePage
		if end > len(keys) {
			end = len(keys)
		}
		var b strings.Builder
		b.WriteString(`<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">`)
		for _, k := range keys[from:end] {
			o := f.objects[bucket][k]
			sum := md5.Sum(o.data)
			fmt.Fprintf(&b, `<Contents><Key>%s</Key><Size>%d</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>"%s"</ETag></Contents>`,
				strings.ReplaceAll(k, "&", "&amp;"), len(o.data), hex.EncodeToString(sum[:]))
		}
		truncated := end < len(keys)
		fmt.Fprintf(&b, "<IsTruncated>%v</IsTruncated>", truncated)
		if truncated {
			fmt.Fprintf(&b, "<NextContinuationToken>%d</NextContinuationToken>", end)
		}
		b.WriteString("</ListBucketResult>")
		w.Header().Set("Content-Type", "application/xml")
		io.WriteString(w, b.String())
	case r.Method == http.MethodPut:
		if f.objects[bucket] == nil {
			f.objects[bucket] = map[string]fakeObject{}
		}
		f.objects[bucket][key] = fakeObject{data: body, contentType: r.Header.Get("Content-Type")}
		etag := "00000000000000000000000000000000"
		if key != "bad-etag" {
			sum := md5.Sum(body)
			etag = hex.EncodeToString(sum[:])
		}
		w.Header().Set("ETag", `"`+etag+`"`)
		w.WriteHeader(200)
	case r.Method == http.MethodDelete:
		delete(f.objects[bucket], key)
		w.WriteHeader(204)
	default:
		o, ok := f.objects[bucket][key]
		if !ok {
			xmlErr(404, "NoSuchKey", "The specified key does not exist.")
			return
		}
		sum := md5.Sum(o.data)
		w.Header().Set("ETag", `"`+hex.EncodeToString(sum[:])+`"`)
		w.Header().Set("Content-Type", o.contentType)
		w.Header().Set("Last-Modified", "Thu, 23 Oct 2026 12:00:00 GMT")
		w.Header().Set("Content-Length", strconv.Itoa(len(o.data)))
		w.WriteHeader(200)
		if r.Method != http.MethodHead {
			w.Write(o.data)
		}
	}
}

func testClient(t *testing.T, srv *httptest.Server) *Client {
	t.Helper()
	c, err := New(Config{
		Endpoint:  srv.URL,
		Region:    "us-east-1",
		AccessKey: fakeAccess,
		SecretKey: fakeSecret,
		Bucket:    "test",
		Timeout:   10 * time.Second,
		Logger:    log.New(io.Discard, "", 0),
	})
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func TestRoundTripWithVerifiedSignatures(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()

	if ok, err := s3.Exists(ctx, "dados/metadata.json"); err != nil || ok {
		t.Fatalf("exists before write: %v %v", ok, err)
	}
	if o, err := s3.Head(ctx, "dados/metadata.json"); err != nil || o != nil {
		t.Fatalf("head before write: %v %v", o, err)
	}
	if _, err := s3.Read(ctx, "dados/metadata.json"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("read before write: %v", err)
	}

	payload := []byte(`{"ultimo_mes":202608}`)
	tr, err := s3.Write(ctx, "dados/metadata.json", payload, "")
	if err != nil {
		t.Fatal(err)
	}
	if tr.Size != 21 || tr.URL() != "s3://test/dados/metadata.json" {
		t.Fatalf("transfer %+v", tr)
	}
	got, err := s3.Read(ctx, "dados/metadata.json")
	if err != nil || !bytes.Equal(got, payload) {
		t.Fatalf("read: %q %v", got, err)
	}
	if got, err := s3.Read(ctx, "s3://test/dados/metadata.json"); err != nil || len(got) != 21 {
		t.Fatalf("read by URL: %d %v", len(got), err)
	}

	o, err := s3.Head(ctx, "dados/metadata.json")
	if err != nil || o == nil {
		t.Fatal(err)
	}
	if o.Size != 21 || o.ContentType != "application/json" || len(o.ETag) != 32 || o.LastModified.IsZero() {
		t.Fatalf("head %+v", o)
	}
	if ok, _ := s3.Exists(ctx, "dados/metadata.json"); !ok {
		t.Fatal("exists after write")
	}
	if err := s3.Delete(ctx, "dados/metadata.json"); err != nil {
		t.Fatal(err)
	}
	if ok, _ := s3.Exists(ctx, "dados/metadata.json"); ok {
		t.Fatal("exists after delete")
	}
	if err := s3.Delete(ctx, "dados/metadata.json"); err != nil {
		t.Fatalf("second delete: %v", err)
	}
}

func TestKeysWithSpecialCharacters(t *testing.T) {
	f, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	key := "pasta/ação & x+y (1).csv"
	if _, err := s3.Write(ctx, key, []byte("a,b\n1,2\n"), ""); err != nil {
		t.Fatal(err)
	}
	got, err := s3.Read(ctx, key)
	if err != nil || string(got) != "a,b\n1,2\n" {
		t.Fatalf("%q %v", got, err)
	}
	listed, err := s3.List(ctx, "pasta/")
	if err != nil || len(listed) != 1 || listed[0].Key != key || listed[0].Size != 8 {
		t.Fatalf("%+v %v", listed, err)
	}
	if listed[0].LastModified.Year() != 2026 {
		t.Fatalf("date %v", listed[0].LastModified)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if o, ok := f.objects["test"][key]; !ok || o.contentType != "text/csv" {
		t.Fatalf("stored %+v %v", o, ok)
	}
}

func TestListingFollowsContinuationTokens(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	for i := 0; i < 5; i++ {
		if _, err := s3.Write(ctx, fmt.Sprintf("p/%d.txt", i), []byte{byte('0' + i)}, ""); err != nil {
			t.Fatal(err)
		}
	}
	s3.Write(ctx, "q/other.txt", []byte("x"), "")
	all, err := s3.List(ctx, "p/")
	if err != nil {
		t.Fatal(err)
	}
	var keys []string
	for _, o := range all {
		keys = append(keys, o.Key)
	}
	if want := "p/0.txt p/1.txt p/2.txt p/3.txt p/4.txt"; strings.Join(keys, " ") != want {
		t.Fatalf("got %v", keys)
	}
	if all, _ := s3.List(ctx, ""); len(all) != 6 {
		t.Fatalf("whole bucket: %d", len(all))
	}
	if all, _ := s3.List(ctx, "s3://test"); len(all) != 6 {
		t.Fatalf("bucket URL: %d", len(all))
	}
	if all, err := s3.List(ctx, "nothing/"); err != nil || len(all) != 0 {
		t.Fatalf("empty: %d %v", len(all), err)
	}
	b, err := s3.Buckets(ctx)
	if err != nil || len(b) != 1 || b[0].Name != "test" || b[0].Created.Year() != 2026 {
		t.Fatalf("%+v %v", b, err)
	}
}

func TestTransientErrorsAreRetriedAndClientErrorsAreNot(t *testing.T) {
	f, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()

	f.mu.Lock()
	f.failFirst = 1
	f.mu.Unlock()
	if _, err := s3.Write(ctx, "flaky.txt", []byte("ok"), ""); err != nil {
		t.Fatal(err)
	}
	if got, err := s3.Read(ctx, "flaky.txt"); err != nil || string(got) != "ok" {
		t.Fatal(err)
	}

	f.mu.Lock()
	f.failFirst = 5
	f.mu.Unlock()
	_, err := s3.Read(ctx, "flaky.txt")
	var e *Error
	if !errors.As(err, &e) || e.Status != 503 || e.Code != "SlowDown" || !errors.Is(err, ErrHTTP) {
		t.Fatalf("after retries: %v", err)
	}
	f.mu.Lock()
	if f.failFirst != 2 {
		t.Fatalf("tries: %d failures left", f.failFirst)
	}
	before := len(f.requests)
	f.mu.Unlock()

	_, err = s3.Read(ctx, "s3://forbidden/x")
	if !errors.Is(err, ErrAccessDenied) {
		t.Fatalf("forbidden: %v", err)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.requests) != before+1 {
		t.Fatalf("403 was retried: %d requests", len(f.requests)-before)
	}
}

func TestUploadIntegrityIsChecked(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	_, err := s3.Write(context.Background(), "bad-etag", []byte("data"), "")
	if !errors.Is(err, ErrIntegrity) {
		t.Fatalf("%v", err)
	}
}

func TestFilesGoUpAndDown(t *testing.T) {
	f, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	dir := t.TempDir()
	src := filepath.Join(dir, "painel.parquet")
	big := make([]byte, 3_000_000)
	for i := range big {
		big[i] = byte(i % 251)
	}
	if err := os.WriteFile(src, big, 0o644); err != nil {
		t.Fatal(err)
	}

	tr, err := s3.Upload(ctx, src, "etl/", "")
	if err != nil {
		t.Fatal(err)
	}
	if tr.Key != "etl/painel.parquet" || tr.Size != int64(len(big)) {
		t.Fatalf("%+v", tr)
	}
	f.mu.Lock()
	if ct := f.objects["test"]["etl/painel.parquet"].contentType; ct != "application/vnd.apache.parquet" {
		t.Fatalf("content type %q", ct)
	}
	f.mu.Unlock()

	dest := filepath.Join(dir, "sub", "copy.parquet")
	tr, err = s3.Download(ctx, "etl/painel.parquet", dest)
	if err != nil {
		t.Fatal(err)
	}
	if tr.Size != int64(len(big)) {
		t.Fatalf("%+v", tr)
	}
	back, _ := os.ReadFile(dest)
	if !bytes.Equal(back, big) {
		t.Fatal("downloaded bytes differ")
	}
	if _, err := os.Stat(dest + ".s3pe-part"); !os.IsNotExist(err) {
		t.Fatal("part file left behind")
	}

	missing := filepath.Join(dir, "missing")
	if _, err := s3.Download(ctx, "etl/missing.parquet", missing); !errors.Is(err, ErrNotFound) {
		t.Fatalf("%v", err)
	}
	if _, err := os.Stat(missing); !os.IsNotExist(err) {
		t.Fatal("file created for a missing object")
	}
	if _, err := os.Stat(missing + ".s3pe-part"); !os.IsNotExist(err) {
		t.Fatal("part file left behind after 404")
	}
}

func TestConfigErrors(t *testing.T) {
	if _, err := New(Config{Endpoint: "127.0.0.1:1", AccessKey: "a", SecretKey: "b"}); !errors.Is(err, ErrConfig) {
		t.Fatalf("bad endpoint: %v", err)
	}
	if _, err := New(Config{Endpoint: "https://x.org/bucket", AccessKey: "a", SecretKey: "b"}); !errors.Is(err, ErrConfig) {
		t.Fatalf("endpoint with path: %v", err)
	}
	t.Setenv("AWS_ACCESS_KEY_ID", "")
	t.Setenv("AWS_SECRET_ACCESS_KEY", "")
	if _, err := New(Config{}); !errors.Is(err, ErrConfig) {
		t.Fatalf("no credentials: %v", err)
	}
	c, err := New(Config{Endpoint: "https://x.org:443/", AccessKey: "a", SecretKey: "b"})
	if err != nil {
		t.Fatal(err)
	}
	if c.Endpoint() != "https://x.org" || c.host != "x.org" {
		t.Fatalf("%s %s", c.Endpoint(), c.host)
	}
	c2, _ := New(Config{Endpoint: "http://127.0.0.1:9003", AccessKey: "a", SecretKey: "b"})
	if c2.host != "127.0.0.1:9003" {
		t.Fatal(c2.host)
	}
	if _, err := c.Read(context.Background(), "x"); !errors.Is(err, ErrConfig) {
		t.Fatalf("no bucket: %v", err)
	}
	if _, err := c.Read(context.Background(), "s3://b/"); !errors.Is(err, ErrConfig) {
		t.Fatalf("no key: %v", err)
	}
	if _, err := c.Read(context.Background(), "s3:///k"); !errors.Is(err, ErrConfig) {
		t.Fatalf("empty bucket: %v", err)
	}
	b, k, _ := c.Resolve("s3://other")
	if b != "other" || k != "" {
		t.Fatal(b, k)
	}
	cb, _ := New(Config{Endpoint: "https://x.org", AccessKey: "a", SecretKey: "b", Bucket: "iaempe"})
	if b, k, _ := cb.Resolve("/dados/x.json"); b != "iaempe" || k != "dados/x.json" {
		t.Fatal(b, k)
	}
}

func TestContextCancellation(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := s3.Read(ctx, "x")
	if !errors.Is(err, ErrTransport) || !errors.Is(err, context.Canceled) {
		t.Fatalf("%v", err)
	}
}

func TestSizesAndTypes(t *testing.T) {
	cases := map[int64]string{0: "0 B", 999: "999 B", 1234: "1.2 kB", 12_345_678: "12.3 MB", 512: "512 B"}
	for n, want := range cases {
		if got := FormatSize(n); got != want {
			t.Errorf("FormatSize(%d) = %q, want %q", n, got, want)
		}
	}
	types := map[string]string{
		"dados/series.csv.gz": "application/gzip",
		"a/b.PARQUET":         "application/vnd.apache.parquet",
		"noext":               "application/octet-stream",
		"x.geojson":           "application/geo+json",
		"x.json":              "application/json",
	}
	for k, want := range types {
		if got := ContentTypeFor(k); got != want {
			t.Errorf("ContentTypeFor(%q) = %q, want %q", k, got, want)
		}
	}
}

func TestErrorMessages(t *testing.T) {
	e := &Error{kind: ErrNotFound, Bucket: "b", Key: "k"}
	if e.Error() != "not found: s3://b/k" {
		t.Fatal(e.Error())
	}
	e = &Error{kind: ErrHTTP, Status: 503, Code: "SlowDown", Message: "try later", Bucket: "b", Key: "k"}
	if e.Error() != "HTTP 503 SlowDown: try later (s3://b/k)" {
		t.Fatal(e.Error())
	}
}
