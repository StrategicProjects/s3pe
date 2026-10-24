package s3pe

import (
	"bytes"
	"context"
	"crypto/md5"
	"encoding/base64"
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

// A fake S3 on localhost: checks every signature (header style, and query
// style for pre-signed URLs) with the package's own signer, keeps objects,
// metadata and multipart uploads in memory, and misbehaves on demand.

const (
	fakeAccess = "GKtest"
	fakeSecret = "secret-of-the-fake"
	fakePage   = 2
)

type fakeS3 struct {
	mu         sync.Mutex
	objects    map[string]map[string]fakeObject // bucket -> key -> object
	uploads    map[string]map[int][]byte        // uploadId -> part number -> bytes
	inits      map[string]fakeObject            // uploadId -> content type and metadata of the initiate request
	requests   []string
	failFirst  int
	failPart   int
	nextUpload int
}

type fakeObject struct {
	data        []byte
	contentType string
	metadata    map[string]string
	etag        string
}

func newFake(t *testing.T) (*fakeS3, *httptest.Server) {
	f := &fakeS3{objects: map[string]map[string]fakeObject{}, uploads: map[string]map[int][]byte{}, inits: map[string]fakeObject{}}
	srv := httptest.NewServer(f)
	t.Cleanup(srv.Close)
	return f, srv
}

func md5hex(b []byte) string {
	sum := md5.Sum(b)
	return hex.EncodeToString(sum[:])
}

func (f *fakeS3) put(bucket, key string, o fakeObject) {
	if f.objects[bucket] == nil {
		f.objects[bucket] = map[string]fakeObject{}
	}
	f.objects[bucket][key] = o
}

func (f *fakeS3) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	creds := credentials{accessKey: fakeAccess, secretKey: fakeSecret}
	var query, plain []pair
	for k, vs := range r.URL.Query() {
		for _, v := range vs {
			query = append(query, pair{k, v})
			if !strings.HasPrefix(k, "X-Amz-") {
				plain = append(plain, pair{k, v})
			}
		}
	}
	q := r.URL.Query()

	var signed bool
	if sig := q.Get("X-Amz-Signature"); sig != "" {
		expires, _ := strconv.ParseInt(q.Get("X-Amz-Expires"), 10, 64)
		want := presign(presignRequest{
			method:  r.Method,
			host:    r.Host,
			path:    r.URL.EscapedPath(),
			query:   plain,
			amzDate: q.Get("X-Amz-Date"),
			expires: expires,
		}, creds, "us-east-1")
		signed = strings.HasSuffix(want, "X-Amz-Signature="+sig)
	} else {
		if got := r.Header.Get("x-amz-content-sha256"); got != sha256Hex(body) {
			http.Error(w, "payload hash "+got, http.StatusBadRequest)
			return
		}
		var extra []pair
		for name, vs := range r.Header {
			n := strings.ToLower(name)
			if strings.HasPrefix(n, "x-amz-") && n != "x-amz-date" && n != "x-amz-content-sha256" {
				extra = append(extra, pair{n, vs[0]})
			}
		}
		want := ""
		for _, h := range sign(signRequest{
			method:       r.Method,
			host:         r.Host,
			path:         r.URL.EscapedPath(),
			query:        query,
			payloadHash:  r.Header.Get("x-amz-content-sha256"),
			amzDate:      r.Header.Get("x-amz-date"),
			extraHeaders: extra,
		}, creds, "us-east-1") {
			if h.name == "authorization" {
				want = h.value
			}
		}
		signed = r.Header.Get("Authorization") == want
	}

	segs := strings.SplitN(strings.TrimPrefix(r.URL.EscapedPath(), "/"), "/", 2)
	bucket, _ := url.PathUnescape(segs[0])
	key := ""
	if len(segs) == 2 {
		key, _ = url.PathUnescape(segs[1])
	}
	metaOf := func() map[string]string {
		m := map[string]string{}
		for name, vs := range r.Header {
			if short, ok := strings.CutPrefix(strings.ToLower(name), "x-amz-meta-"); ok {
				m[short] = vs[0]
			}
		}
		return m
	}

	f.mu.Lock()
	defer f.mu.Unlock()
	f.requests = append(f.requests, r.Method+" "+r.URL.RequestURI())

	xmlErr := func(status int, code, msg string) {
		w.Header().Set("Content-Type", "application/xml")
		w.WriteHeader(status)
		fmt.Fprintf(w, "<Error><Code>%s</Code><Message>%s</Message></Error>", code, msg)
	}
	xmlOK := func(s string) {
		w.Header().Set("Content-Type", "application/xml")
		io.WriteString(w, s)
	}
	_, hasUploads := q["uploads"]
	_, hasDelete := q["delete"]
	switch {
	case !signed:
		xmlErr(403, "SignatureDoesNotMatch", "bad signature")
	case bucket == "forbidden":
		xmlErr(403, "AccessDenied", "Access Denied")
	case strings.Contains(key, "flaky") && f.failFirst > 0:
		f.failFirst--
		xmlErr(503, "SlowDown", "try later")
	case bucket == "":
		xmlOK(`<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>`)
	case r.Method == http.MethodPost && key == "" && hasDelete:
		if got := r.Header.Get("Content-MD5"); got != base64.StdEncoding.EncodeToString(func() []byte { s := md5.Sum(body); return s[:] }()) {
			xmlErr(400, "InvalidDigest", "Content-MD5 "+got)
			return
		}
		var b strings.Builder
		b.WriteString("<DeleteResult>")
		for _, part := range strings.Split(string(body), "<Key>")[1:] {
			k := strings.SplitN(part, "</Key>", 2)[0]
			k = strings.NewReplacer("&amp;", "&", "&lt;", "<", "&gt;", ">", "&#39;", "'", "&#34;", `"`).Replace(k)
			if k == "undeletable" {
				b.WriteString("<Error><Key>undeletable</Key><Code>AccessDenied</Code><Message>nope</Message></Error>")
			} else {
				delete(f.objects[bucket], k)
			}
		}
		b.WriteString("</DeleteResult>")
		xmlOK(b.String())
	case r.Method == http.MethodGet && key == "":
		prefix := q.Get("prefix")
		delimiter := q.Get("delimiter")
		from, _ := strconv.Atoi(q.Get("continuation-token"))
		var keys, prefixes []string
		seen := map[string]bool{}
		for k := range f.objects[bucket] {
			if !strings.HasPrefix(k, prefix) {
				continue
			}
			rest := k[len(prefix):]
			if delimiter != "" && strings.Contains(rest, delimiter) {
				cp := prefix + strings.SplitN(rest, delimiter, 2)[0] + delimiter
				if !seen[cp] {
					seen[cp] = true
					prefixes = append(prefixes, cp)
				}
				continue
			}
			keys = append(keys, k)
		}
		sort.Strings(keys)
		sort.Strings(prefixes)
		end := from + fakePage
		if end > len(keys) {
			end = len(keys)
		}
		var b strings.Builder
		b.WriteString(`<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">`)
		for _, k := range keys[from:end] {
			o := f.objects[bucket][k]
			fmt.Fprintf(&b, `<Contents><Key>%s</Key><Size>%d</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>"%s"</ETag></Contents>`,
				strings.ReplaceAll(k, "&", "&amp;"), len(o.data), o.etag)
		}
		if from == 0 {
			for _, p := range prefixes {
				fmt.Fprintf(&b, "<CommonPrefixes><Prefix>%s</Prefix></CommonPrefixes>", p)
			}
		}
		truncated := end < len(keys)
		fmt.Fprintf(&b, "<IsTruncated>%v</IsTruncated>", truncated)
		if truncated {
			fmt.Fprintf(&b, "<NextContinuationToken>%d</NextContinuationToken>", end)
		}
		b.WriteString("</ListBucketResult>")
		xmlOK(b.String())
	case r.Method == http.MethodPost && hasUploads:
		f.nextUpload++
		id := fmt.Sprintf("upload-%d", f.nextUpload)
		f.uploads[id] = map[int][]byte{}
		f.inits[id] = fakeObject{contentType: r.Header.Get("Content-Type"), metadata: metaOf()}
		xmlOK(fmt.Sprintf("<InitiateMultipartUploadResult><UploadId>%s</UploadId></InitiateMultipartUploadResult>", id))
	case r.Method == http.MethodPut && q.Get("partNumber") != "":
		n, _ := strconv.Atoi(q.Get("partNumber"))
		id := q.Get("uploadId")
		parts, ok := f.uploads[id]
		switch {
		case f.failPart == n:
			xmlErr(500, "InternalError", "part refused")
		case !ok:
			xmlErr(404, "NoSuchUpload", "no such upload")
		case r.Header.Get("Content-Type") == "":
			xmlErr(500, "InternalError", "untyped part body")
		default:
			parts[n] = body
			w.Header().Set("ETag", `"`+md5hex(body)+`"`)
			w.WriteHeader(200)
		}
	case r.Method == http.MethodPost && q.Get("uploadId") != "":
		id := q.Get("uploadId")
		parts, ok := f.uploads[id]
		if !ok {
			xmlErr(404, "NoSuchUpload", "no such upload")
			return
		}
		var numbers []int
		for n := range parts {
			numbers = append(numbers, n)
		}
		sort.Ints(numbers)
		var data []byte
		var md5s []string
		for _, n := range numbers {
			data = append(data, parts[n]...)
			md5s = append(md5s, md5hex(parts[n]))
		}
		etag := MultipartETag(md5s)
		init := f.inits[id]
		f.put(bucket, key, fakeObject{data: data, contentType: init.contentType, metadata: init.metadata, etag: etag})
		delete(f.uploads, id)
		delete(f.inits, id)
		xmlOK(fmt.Sprintf(`<CompleteMultipartUploadResult><ETag>"%s"</ETag></CompleteMultipartUploadResult>`, etag))
	case r.Method == http.MethodDelete && q.Get("uploadId") != "":
		delete(f.uploads, q.Get("uploadId"))
		delete(f.inits, q.Get("uploadId"))
		w.WriteHeader(204)
	case r.Method == http.MethodPut && r.Header.Get("x-amz-copy-source") != "":
		src := strings.SplitN(strings.TrimPrefix(r.Header.Get("x-amz-copy-source"), "/"), "/", 2)
		sb, _ := url.PathUnescape(src[0])
		sk, _ := url.PathUnescape(src[1])
		o, ok := f.objects[sb][sk]
		if !ok {
			xmlErr(404, "NoSuchKey", "no source")
			return
		}
		f.put(bucket, key, o)
		xmlOK(fmt.Sprintf(`<CopyObjectResult><ETag>"%s"</ETag></CopyObjectResult>`, o.etag))
	case r.Method == http.MethodPut:
		etag := md5hex(body)
		if key == "bad-etag" {
			etag = "00000000000000000000000000000000"
		}
		f.put(bucket, key, fakeObject{data: body, contentType: r.Header.Get("Content-Type"), metadata: metaOf(), etag: etag})
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
		w.Header().Set("ETag", `"`+o.etag+`"`)
		w.Header().Set("Content-Type", o.contentType)
		w.Header().Set("Last-Modified", "Thu, 23 Oct 2026 12:00:00 GMT")
		w.Header().Set("Content-Length", strconv.Itoa(len(o.data)))
		for k, v := range o.metadata {
			w.Header().Set("x-amz-meta-"+k, v)
		}
		w.WriteHeader(200)
		if r.Method != http.MethodHead {
			w.Write(o.data)
		}
	}
}

func testClient(t *testing.T, srv *httptest.Server) *Client {
	t.Helper()
	return testClientWith(t, srv, Config{})
}

func testClientWith(t *testing.T, srv *httptest.Server, cfg Config) *Client {
	t.Helper()
	cfg.Endpoint = srv.URL
	cfg.Region = "us-east-1"
	cfg.AccessKey, cfg.SecretKey = fakeAccess, fakeSecret
	if cfg.Bucket == "" {
		cfg.Bucket = "test"
	}
	cfg.Timeout = 10 * time.Second
	cfg.Logger = log.New(io.Discard, "", 0)
	c, err := New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func (f *fakeS3) stored(bucket, key string) (fakeObject, bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	o, ok := f.objects[bucket][key]
	return o, ok
}

func (f *fakeS3) count(substr string) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	n := 0
	for _, r := range f.requests {
		if strings.Contains(r, substr) {
			n++
		}
	}
	return n
}

func TestRoundTripWithVerifiedSignatures(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()

	if ok, err := s3.Exists(ctx, "data/manifest.json"); err != nil || ok {
		t.Fatalf("exists before write: %v %v", ok, err)
	}
	if o, err := s3.Head(ctx, "data/manifest.json"); err != nil || o != nil {
		t.Fatalf("head before write: %v %v", o, err)
	}
	if _, err := s3.Read(ctx, "data/manifest.json"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("read before write: %v", err)
	}

	payload := []byte(`{"version":202608}`)
	tr, err := s3.Write(ctx, "data/manifest.json", payload, "")
	if err != nil {
		t.Fatal(err)
	}
	if tr.Size != 18 || tr.URL() != "s3://test/data/manifest.json" {
		t.Fatalf("transfer %+v", tr)
	}
	got, err := s3.Read(ctx, "data/manifest.json")
	if err != nil || !bytes.Equal(got, payload) {
		t.Fatalf("read: %q %v", got, err)
	}
	if got, err := s3.Read(ctx, "s3://test/data/manifest.json"); err != nil || len(got) != 18 {
		t.Fatalf("read by URL: %d %v", len(got), err)
	}

	o, err := s3.Head(ctx, "data/manifest.json")
	if err != nil || o == nil {
		t.Fatal(err)
	}
	if o.Size != 18 || o.ContentType != "application/json" || len(o.ETag) != 32 || o.LastModified.IsZero() {
		t.Fatalf("head %+v", o)
	}
	if ok, _ := s3.Exists(ctx, "data/manifest.json"); !ok {
		t.Fatal("exists after write")
	}
	if err := s3.Delete(ctx, "data/manifest.json"); err != nil {
		t.Fatal(err)
	}
	if ok, _ := s3.Exists(ctx, "data/manifest.json"); ok {
		t.Fatal("exists after delete")
	}
	if err := s3.Delete(ctx, "data/manifest.json"); err != nil {
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
	if o, ok := f.stored("test", key); !ok || o.contentType != "text/csv" {
		t.Fatalf("stored %+v %v", o, ok)
	}
}

func TestListingFollowsContinuationTokensAndDelimiters(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	for i := 0; i < 5; i++ {
		if _, err := s3.Write(ctx, fmt.Sprintf("p/%d.txt", i), []byte{byte('0' + i)}, ""); err != nil {
			t.Fatal(err)
		}
	}
	s3.Write(ctx, "p/sub/a.txt", []byte("a"), "")
	s3.Write(ctx, "p/sub/deeper/b.txt", []byte("b"), "")
	s3.Write(ctx, "q/other.txt", []byte("x"), "")
	all, err := s3.List(ctx, "p/")
	if err != nil || len(all) != 7 {
		t.Fatalf("%d %v", len(all), err)
	}
	if all, _ := s3.List(ctx, ""); len(all) != 8 {
		t.Fatalf("whole bucket: %d", len(all))
	}
	if all, _ := s3.List(ctx, "s3://test"); len(all) != 8 {
		t.Fatalf("bucket URL: %d", len(all))
	}
	if all, err := s3.List(ctx, "nothing/"); err != nil || len(all) != 0 {
		t.Fatalf("empty: %d %v", len(all), err)
	}
	dir, err := s3.ListDir(ctx, "p/")
	if err != nil || len(dir.Objects) != 5 || strings.Join(dir.Prefixes, ",") != "p/sub/" {
		t.Fatalf("list dir: %+v %v", dir, err)
	}
	root, err := s3.ListDir(ctx, "")
	if err != nil || len(root.Objects) != 0 || strings.Join(root.Prefixes, ",") != "p/,q/" {
		t.Fatalf("root dir: %+v %v", root, err)
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
	src := filepath.Join(dir, "sales.parquet")
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
	if tr.Key != "etl/sales.parquet" || tr.Size != int64(len(big)) {
		t.Fatalf("%+v", tr)
	}
	if o, _ := f.stored("test", "etl/sales.parquet"); o.contentType != "application/vnd.apache.parquet" {
		t.Fatalf("content type %q", o.contentType)
	}

	dest := filepath.Join(dir, "sub", "copy.parquet")
	tr, err = s3.Download(ctx, "etl/sales.parquet", dest)
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

func TestMultipartUploadFromMemoryAndFile(t *testing.T) {
	f, srv := newFake(t)
	s3 := testClientWith(t, srv, Config{MultipartThreshold: 1 << 20, PartSize: 5 << 20, Parallel: 3})
	ctx := context.Background()
	big := make([]byte, 11<<20+7)
	for i := range big {
		big[i] = byte(i % 253)
	}
	tr, err := s3.WriteWith(ctx, "big/mem.bin", big, PutOptions{ContentType: "application/x-test", Metadata: map[string]string{"Origin": "test"}})
	if err != nil {
		t.Fatal(err)
	}
	if tr.Size != int64(len(big)) || !strings.HasSuffix(tr.ETag, "-3") {
		t.Fatalf("%+v", tr)
	}
	o, ok := f.stored("test", "big/mem.bin")
	if !ok || !bytes.Equal(o.data, big) || o.contentType != "application/x-test" || o.metadata["origin"] != "test" {
		t.Fatalf("stored: ok=%v ct=%q meta=%v same=%v", ok, o.contentType, o.metadata, bytes.Equal(o.data, big))
	}
	if n := f.count("partNumber"); n != 3 {
		t.Fatalf("%d part requests", n)
	}
	f.mu.Lock()
	if len(f.uploads) != 0 {
		t.Fatal("upload left open")
	}
	f.mu.Unlock()
	back, err := s3.Read(ctx, "big/mem.bin")
	if err != nil || !bytes.Equal(back, big) {
		t.Fatal("read back differs")
	}
	h, _ := s3.Head(ctx, "big/mem.bin")
	if h.Metadata["origin"] != "test" {
		t.Fatalf("head metadata %v", h.Metadata)
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "big.bin")
	os.WriteFile(path, big, 0o644)
	tr, err = s3.Upload(ctx, path, "big/file.bin", "")
	if err != nil || !strings.HasSuffix(tr.ETag, "-3") {
		t.Fatalf("%+v %v", tr, err)
	}
	if o, _ := f.stored("test", "big/file.bin"); !bytes.Equal(o.data, big) {
		t.Fatal("file upload differs")
	}
	want, err := ExpectedETag(path, 1<<20, 5<<20)
	if err != nil || want != tr.ETag {
		t.Fatalf("expected etag %s vs %s (%v)", want, tr.ETag, err)
	}
	small, _ := ExpectedETag(path, 1<<30, 5<<20)
	if small != md5hex(big) {
		t.Fatal("small-file etag is not the md5")
	}

	f.mu.Lock()
	f.failPart = 2
	f.mu.Unlock()
	once := testClientWith(t, srv, Config{MultipartThreshold: 1 << 20, PartSize: 5 << 20, MaxTries: 1})
	_, err = once.Write(ctx, "big/fails.bin", big, "")
	var e *Error
	if !errors.As(err, &e) || e.Status != 500 || !errors.Is(err, ErrHTTP) {
		t.Fatalf("failing part: %v", err)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.uploads) != 0 {
		t.Fatal("failed upload not aborted")
	}
	if _, ok := f.objects["test"]["big/fails.bin"]; ok {
		t.Fatal("object created from a failed upload")
	}
	aborted := false
	for _, r := range f.requests {
		if strings.HasPrefix(r, "DELETE") && strings.Contains(r, "uploadId") {
			aborted = true
		}
	}
	if !aborted {
		t.Fatal("no abort request")
	}
}

func TestCopyBatchDeleteMetadataAndHeaders(t *testing.T) {
	f, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	opts := PutOptions{Metadata: map[string]string{"Author": "ana"}, Headers: map[string]string{"Cache-Control": "max-age=60", "x-amz-storage-class": "STANDARD"}}
	if _, err := s3.WriteWith(ctx, "a/one.txt", []byte("1"), opts); err != nil {
		t.Fatal(err)
	}
	h, err := s3.Head(ctx, "a/one.txt")
	if err != nil || h.Metadata["author"] != "ana" {
		t.Fatalf("%+v %v", h, err)
	}

	tr, err := s3.Copy(ctx, "a/one.txt", "s3://test/b/one-copy.txt")
	if err != nil || tr.Key != "b/one-copy.txt" || tr.Size != 1 {
		t.Fatalf("%+v %v", tr, err)
	}
	if got, _ := s3.Read(ctx, "b/one-copy.txt"); string(got) != "1" {
		t.Fatal("copy differs")
	}
	if h, _ := s3.Head(ctx, "b/one-copy.txt"); h.Metadata["author"] != "ana" {
		t.Fatal("copy lost the metadata")
	}
	if _, err := s3.Copy(ctx, "a/missing", "b/x"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("copy of a missing object: %v", err)
	}

	for i := 0; i < 5; i++ {
		s3.Write(ctx, fmt.Sprintf("d/%d.txt", i), []byte("x"), "")
	}
	s3.Write(ctx, "d/a&b.txt", []byte("x"), "")
	listed, _ := s3.List(ctx, "d/")
	var keys []string
	for _, o := range listed {
		keys = append(keys, "s3://test/"+o.Key)
	}
	if len(keys) != 6 {
		t.Fatalf("%d keys", len(keys))
	}
	if err := s3.DeleteMany(ctx, keys); err != nil {
		t.Fatal(err)
	}
	if left, _ := s3.List(ctx, "d/"); len(left) != 0 {
		t.Fatalf("left %+v", left)
	}
	if n := f.count("?delete"); n != 1 {
		t.Fatalf("%d DeleteObjects requests", n)
	}

	s3.Write(ctx, "undeletable", []byte("x"), "")
	err = s3.DeleteMany(ctx, []string{"undeletable", "a/one.txt"})
	var e *Error
	if !errors.As(err, &e) || e.Code != "DeleteErrors" || !errors.Is(err, ErrHTTP) {
		t.Fatalf("partial failure: %v", err)
	}
	if ok, _ := s3.Exists(ctx, "a/one.txt"); ok {
		t.Fatal("the deletable key stayed")
	}
	if err := s3.DeleteMany(ctx, nil); err != nil {
		t.Fatalf("empty batch: %v", err)
	}
}

func TestPresignedURLsWorkWithoutCredentials(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	s3.Write(ctx, "pub/report.pdf", []byte("%PDF"), "")
	u, err := s3.Presign("pub/report.pdf", "get", 600*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(u, srv.URL+"/test/pub/report.pdf?X-Amz-Algorithm=AWS4-HMAC-SHA256&") || !strings.Contains(u, "X-Amz-Expires=600") {
		t.Fatal(u)
	}
	resp, err := http.Get(u)
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 || string(body) != "%PDF" {
		t.Fatalf("%d %q", resp.StatusCode, body)
	}

	put, err := s3.Presign("pub/incoming.txt", "PUT", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	req, _ := http.NewRequest(http.MethodPut, put, strings.NewReader("hello"))
	req.Header.Set("x-amz-content-sha256", sha256Hex([]byte("hello")))
	resp, err = http.DefaultClient.Do(req)
	if err != nil || resp.StatusCode != 200 {
		t.Fatalf("%v %v", resp, err)
	}
	resp.Body.Close()
	if got, _ := s3.Read(ctx, "pub/incoming.txt"); string(got) != "hello" {
		t.Fatalf("%q", got)
	}

	resp, _ = http.Get(strings.Replace(u, "report.pdf", "other.pdf", 1))
	resp.Body.Close()
	if resp.StatusCode != 403 {
		t.Fatalf("tampered URL accepted: %d", resp.StatusCode)
	}
	if _, err := s3.Presign("k", "DELETE", time.Second); !errors.Is(err, ErrConfig) {
		t.Fatalf("%v", err)
	}
	long, _ := s3.Presign("k", "GET", 30*24*time.Hour)
	if !strings.Contains(long, "X-Amz-Expires=604800") {
		t.Fatal("expires not clamped")
	}
}

func TestSyncUpAndDownByETag(t *testing.T) {
	_, srv := newFake(t)
	s3 := testClient(t, srv)
	ctx := context.Background()
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "sub"), 0o755)
	os.WriteFile(filepath.Join(dir, "a.csv"), []byte("a"), 0o644)
	os.WriteFile(filepath.Join(dir, "sub", "b.csv"), []byte("b"), 0o644)
	os.WriteFile(filepath.Join(dir, "manifest.json"), []byte("{}"), 0o644)
	os.WriteFile(filepath.Join(dir, "x.s3pe-part"), []byte("tmp"), 0o644)

	opts := SyncOptions{Delete: true, Last: "manifest.json"}
	r, err := s3.SyncUp(ctx, dir, "s3://test/out", opts)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(r.Transferred, ",") != "a.csv,sub/b.csv,manifest.json" || len(r.Skipped) != 0 || len(r.Deleted) != 0 {
		t.Fatalf("%+v", r)
	}
	if listed, _ := s3.List(ctx, "out/"); len(listed) != 3 {
		t.Fatalf("%+v", listed)
	}

	r, err = s3.SyncUp(ctx, dir, "out/", opts)
	if err != nil || len(r.Transferred) != 0 || len(r.Skipped) != 3 {
		t.Fatalf("%+v %v", r, err)
	}
	os.WriteFile(filepath.Join(dir, "a.csv"), []byte("a2"), 0o644)
	s3.Write(ctx, "out/stale.txt", []byte("old"), "")
	r, err = s3.SyncUp(ctx, dir, "out/", opts)
	if err != nil || strings.Join(r.Transferred, ",") != "a.csv" || strings.Join(r.Deleted, ",") != "stale.txt" {
		t.Fatalf("%+v %v", r, err)
	}
	if ok, _ := s3.Exists(ctx, "out/stale.txt"); ok {
		t.Fatal("stale object kept")
	}
	if _, err := s3.SyncUp(ctx, dir, "out/", SyncOptions{Last: "nope"}); !errors.Is(err, ErrConfig) {
		t.Fatalf("missing last: %v", err)
	}

	down := t.TempDir()
	os.WriteFile(filepath.Join(down, "extra.txt"), []byte("x"), 0o644)
	r, err = s3.SyncDown(ctx, "out/", down, SyncOptions{Delete: true})
	if err != nil || len(r.Transferred) != 3 || strings.Join(r.Deleted, ",") != "extra.txt" {
		t.Fatalf("%+v %v", r, err)
	}
	if b, _ := os.ReadFile(filepath.Join(down, "sub", "b.csv")); string(b) != "b" {
		t.Fatal("sub/b.csv")
	}
	if a, _ := os.ReadFile(filepath.Join(down, "a.csv")); string(a) != "a2" {
		t.Fatal("a.csv")
	}
	r, err = s3.SyncDown(ctx, "out/", down, SyncOptions{})
	if err != nil || len(r.Transferred) != 0 || len(r.Skipped) != 3 {
		t.Fatalf("%+v %v", r, err)
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
	if _, err := New(Config{Endpoint: "https://x.org"}); !errors.Is(err, ErrConfig) {
		t.Fatalf("no credentials: %v", err)
	}
	c, err := New(Config{Endpoint: "https://x.org:443/", AccessKey: "a", SecretKey: "b"})
	if err != nil {
		t.Fatal(err)
	}
	if c.Endpoint() != "https://x.org" || c.host != "x.org" {
		t.Fatalf("%s %s", c.Endpoint(), c.host)
	}
	if c.threshold != DefaultMultipartThreshold || c.partSize != DefaultPartSize || c.parallel != DefaultParallel {
		t.Fatalf("defaults %d %d %d", c.threshold, c.partSize, c.parallel)
	}
	c2, _ := New(Config{Endpoint: "http://127.0.0.1:9003", AccessKey: "a", SecretKey: "b", PartSize: 1})
	if c2.host != "127.0.0.1:9003" || c2.partSize != MinPartSize {
		t.Fatal(c2.host, c2.partSize)
	}
	t.Setenv("S3PE_MULTIPART_THRESHOLD", "1048576")
	t.Setenv("S3PE_PART_SIZE", "6291456")
	t.Setenv("S3PE_PARALLEL", "2")
	c3, _ := New(Config{Endpoint: "https://x.org", AccessKey: "a", SecretKey: "b"})
	if c3.threshold != 1<<20 || c3.partSize != 6<<20 || c3.parallel != 2 {
		t.Fatal(c3.threshold, c3.partSize, c3.parallel)
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
	cb, _ := New(Config{Endpoint: "https://x.org", AccessKey: "a", SecretKey: "b", Bucket: "data"})
	if b, k, _ := cb.Resolve("/raw/x.json"); b != "data" || k != "raw/x.json" {
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

func TestSizesTypesAndETags(t *testing.T) {
	cases := map[int64]string{0: "0 B", 999: "999 B", 1234: "1.2 kB", 12_345_678: "12.3 MB", 512: "512 B"}
	for n, want := range cases {
		if got := FormatSize(n); got != want {
			t.Errorf("FormatSize(%d) = %q, want %q", n, got, want)
		}
	}
	types := map[string]string{
		"raw/series.csv.gz": "application/gzip",
		"a/b.PARQUET":       "application/vnd.apache.parquet",
		"noext":             "application/octet-stream",
		"x.geojson":         "application/geo+json",
		"x.json":            "application/json",
	}
	for k, want := range types {
		if got := ContentTypeFor(k); got != want {
			t.Errorf("ContentTypeFor(%q) = %q, want %q", k, got, want)
		}
	}
	// md5("a") and md5("b")
	e := MultipartETag([]string{"0cc175b9c0f1b6a831c399e269772661", "92eb5ffee6ae2fec3ad71c777531578f"})
	if len(e) != 34 || !strings.HasSuffix(e, "-2") {
		t.Fatal(e)
	}
	if normalizePrefix("a") != "a/" || normalizePrefix("a/") != "a/" || normalizePrefix("") != "" {
		t.Fatal("normalizePrefix")
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

func TestEndpointIsRequired(t *testing.T) {
	t.Setenv("S3PE_ENDPOINT", "")
	t.Setenv("AWS_ENDPOINT_URL", "")
	_, err := New(Config{AccessKey: "a", SecretKey: "b"})
	if !errors.Is(err, ErrConfig) {
		t.Fatalf("want ErrConfig without an endpoint, got %v", err)
	}
}
