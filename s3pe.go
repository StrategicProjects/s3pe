package s3pe

import (
	"bytes"
	"context"
	"crypto/md5"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"fmt"
	"hash"
	"io"
	"io/fs"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	// DefaultRegion is the signing region used when none is given.
	DefaultRegion = "us-east-1"
	// DefaultTimeout is the time one call may take before it is abandoned.
	DefaultTimeout = 300 * time.Second
	// DefaultMaxTries is the number of attempts per call: the first and two retries.
	DefaultMaxTries = 3
	// DefaultMultipartThreshold: bodies larger than this go up as a multipart upload (D15).
	DefaultMultipartThreshold int64 = 64 * 1024 * 1024
	// DefaultPartSize is the size of each part of a multipart upload (D15).
	DefaultPartSize int64 = 16 * 1024 * 1024
	// DefaultParallel is the number of parts sent at the same time (D15).
	DefaultParallel = 4
	// MinPartSize is the smallest part S3 accepts, except for the last one.
	MinPartSize int64 = 5 * 1024 * 1024
	// MaxPresignSecs is the longest validity of a pre-signed URL (seven days).
	MaxPresignSecs int64 = 7 * 24 * 3600

	version   = "0.3.0"
	userAgent = "s3pe-go/" + version
)

// Sentinel errors, to be tested with errors.Is.
var (
	// ErrConfig: missing endpoint, credentials or bucket, bad endpoint.
	ErrConfig = errors.New("configuration")
	// ErrTransport: the server could not be reached, or the connection broke (retried first).
	ErrTransport = errors.New("transport")
	// ErrNotFound: the object or bucket does not exist (HTTP 404).
	ErrNotFound = errors.New("not found")
	// ErrAccessDenied: the credentials do not allow this (HTTP 403).
	ErrAccessDenied = errors.New("access denied")
	// ErrHTTP: any other HTTP error; the *Error carries status, code and message.
	ErrHTTP = errors.New("http")
	// ErrIntegrity: the bytes that arrived are not the bytes that were sent.
	ErrIntegrity = errors.New("integrity")
)

// Error is what every failed call returns. errors.Is(err, ErrNotFound) and
// the other sentinels tell the kind; errors.As gives the details.
type Error struct {
	// Status is the HTTP status, 0 when no response arrived.
	Status int
	// Code is the S3 error code (NoSuchKey, AccessDenied, …), empty if none.
	Code string
	// Message is the server's or the client's explanation.
	Message string
	// Bucket and Key name what was asked for (empty for bucket-level calls).
	Bucket, Key string

	kind error
	err  error
}

func (e *Error) Error() string {
	where := ""
	if e.Bucket != "" {
		where = " (s3://" + e.Bucket + "/" + e.Key + ")"
	}
	switch e.kind {
	case ErrNotFound:
		if e.Key == "" {
			return "not found: bucket " + e.Bucket
		}
		return "not found: s3://" + e.Bucket + "/" + e.Key
	case ErrAccessDenied:
		s := "access denied: s3://" + e.Bucket + "/" + e.Key
		if e.Message != "" {
			s += " (" + e.Message + ")"
		}
		return s
	case ErrHTTP:
		s := fmt.Sprintf("HTTP %d", e.Status)
		if e.Code != "" {
			s += " " + e.Code
		}
		if e.Message != "" {
			s += ": " + e.Message
		}
		return s + where
	case ErrConfig:
		return "configuration: " + e.Message
	case ErrTransport:
		return "transport: " + e.Message
	case ErrIntegrity:
		return "integrity: " + e.Message
	}
	return e.Message
}

// Is reports whether the error is of the sentinel kind target.
func (e *Error) Is(target error) bool { return target == e.kind }

// Unwrap returns the underlying error (network, file system, context), if any.
func (e *Error) Unwrap() error { return e.err }

func configErr(format string, a ...any) error {
	return &Error{kind: ErrConfig, Message: fmt.Sprintf(format, a...)}
}

func transportErr(err error, format string, a ...any) error {
	return &Error{kind: ErrTransport, Message: fmt.Sprintf(format, a...) + ": " + err.Error(), err: err}
}

func integrityErr(format string, a ...any) error {
	return &Error{kind: ErrIntegrity, Message: fmt.Sprintf(format, a...)}
}

func ioErr(err error, what, path string) error {
	return &Error{kind: ErrTransport, Message: what + " " + path + ": " + err.Error(), err: err}
}

// Object is one object as listed or described by the store.
type Object struct {
	// Key inside the bucket.
	Key string
	// Size in bytes.
	Size int64
	// LastModified is zero when the server's date could not be parsed.
	LastModified time.Time
	// ETag without the surrounding quotes.
	ETag string
	// ContentType is only known after Head.
	ContentType string
	// Metadata is the user metadata (x-amz-meta-*, names without the
	// prefix, lower case), only known after Head.
	Metadata map[string]string
}

// Listing is one level of a bucket: the objects directly under a prefix and
// the common prefixes ("folders") below it (D17).
type Listing struct {
	// Objects directly under the prefix.
	Objects []Object
	// Prefixes are the common prefixes, each ending with the delimiter.
	Prefixes []string
}

// Bucket is a bucket as listed by the store.
type Bucket struct {
	Name    string
	Created time.Time
}

// Transfer is what a completed upload, download or copy reports.
type Transfer struct {
	Bucket, Key string
	// Size is the number of bytes moved.
	Size int64
	// ETag reported by the server, without quotes.
	ETag string
	// Elapsed is the wall time of the whole call, retries included.
	Elapsed time.Duration
}

// URL is s3://bucket/key.
func (t Transfer) URL() string { return "s3://" + t.Bucket + "/" + t.Key }

// PutOptions are the options of WriteWith and UploadWith (D17).
type PutOptions struct {
	// ContentType; guessed from the key when empty.
	ContentType string
	// Metadata is user metadata, sent as x-amz-meta-<name>.
	Metadata map[string]string
	// Headers are any other headers (Cache-Control, Content-Disposition,
	// x-amz-storage-class, x-amz-server-side-encryption, …). x-amz-* headers
	// are signed, the rest are not.
	Headers map[string]string
}

// SyncOptions are the options of SyncUp and SyncDown (D18).
type SyncOptions struct {
	// Delete removes what exists on the destination but not on the source.
	Delete bool
	// Last is the relative path of the file to transfer last (a manifest).
	Last string
}

// SyncReport is what a sync did.
type SyncReport struct {
	// Transferred are the relative paths uploaded or downloaded.
	Transferred []string
	// Skipped are the relative paths already identical on both sides.
	Skipped []string
	// Deleted are the relative paths removed from the destination.
	Deleted []string
	// Elapsed is the wall time.
	Elapsed time.Duration
}

// Config configures a Client. Zero fields are taken from the environment
// (S3PE_ENDPOINT or AWS_ENDPOINT_URL, AWS_REGION or AWS_DEFAULT_REGION,
// AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN, S3PE_BUCKET,
// S3PE_MULTIPART_THRESHOLD, S3PE_PART_SIZE, S3PE_PARALLEL, S3PE_QUIET) and then from the
// defaults.
type Config struct {
	// Endpoint is scheme://host[:port], no path. Required.
	Endpoint string
	// Region is the signing region (default us-east-1; on AWS, the bucket's region).
	Region string
	// AccessKey and SecretKey are required, here or in the environment.
	AccessKey, SecretKey string
	// SessionToken is for temporary credentials.
	SessionToken string
	// Bucket is used when a key is not an s3:// URL.
	Bucket string
	// Timeout limits one call (default 300 s).
	Timeout time.Duration
	// MaxTries is the number of attempts per call (default 3).
	MaxTries int
	// MultipartThreshold: bodies above it go up in parts (default 64 MiB).
	MultipartThreshold int64
	// PartSize of a multipart upload, at least 5 MiB (default 16 MiB).
	PartSize int64
	// Parallel is the number of parts sent at the same time (default 4,
	// or S3PE_PARALLEL).
	Parallel int
	// HTTPClient replaces the default one (which follows no redirects).
	HTTPClient *http.Client
	// Logger receives one line per transfer (default: standard error).
	Logger *log.Logger
	// Quiet silences the transfer lines; warnings about retries still go out.
	Quiet bool
}

// Client is a configured connection to one S3 endpoint. Every method takes
// a key that is either relative to the client's bucket or a full
// s3://bucket/key URL. A Client is safe for concurrent use.
type Client struct {
	endpoint  string
	host      string
	region    string
	creds     credentials
	bucket    string
	timeout   time.Duration
	maxTries  int
	threshold int64
	partSize  int64
	parallel  int
	http      *http.Client
	logger    *log.Logger
	quiet     bool
}

func env(names ...string) string {
	for _, n := range names {
		if v := strings.TrimSpace(os.Getenv(n)); v != "" {
			return v
		}
	}
	return ""
}

func envInt64(name string) int64 {
	v, _ := strconv.ParseInt(env(name), 10, 64)
	return v
}

// New builds a client from cfg, the environment and the defaults. It fails
// without an endpoint, without credentials or with an endpoint that is not
// scheme://host[:port].
func New(cfg Config) (*Client, error) {
	endpoint := cfg.Endpoint
	if endpoint == "" {
		endpoint = env("S3PE_ENDPOINT", "AWS_ENDPOINT_URL")
	}
	if endpoint == "" {
		return nil, &Error{kind: ErrConfig, Message: "no endpoint: set S3PE_ENDPOINT (or AWS_ENDPOINT_URL) to https://host[:port] or Config.Endpoint"}
	}
	endpoint, host, err := parseEndpoint(endpoint)
	if err != nil {
		return nil, err
	}
	region := cfg.Region
	if region == "" {
		region = env("AWS_REGION", "AWS_DEFAULT_REGION")
	}
	if region == "" {
		region = DefaultRegion
	}
	access, secret := cfg.AccessKey, cfg.SecretKey
	if access == "" {
		access = env("AWS_ACCESS_KEY_ID")
	}
	if secret == "" {
		secret = env("AWS_SECRET_ACCESS_KEY")
	}
	if access == "" || secret == "" {
		return nil, configErr("no credentials: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY or pass them in Config")
	}
	token := cfg.SessionToken
	if token == "" {
		token = env("AWS_SESSION_TOKEN")
	}
	bucket := cfg.Bucket
	if bucket == "" {
		bucket = env("S3PE_BUCKET")
	}
	timeout := cfg.Timeout
	if timeout <= 0 {
		timeout = DefaultTimeout
	}
	tries := cfg.MaxTries
	if tries <= 0 {
		tries = DefaultMaxTries
	}
	threshold := cfg.MultipartThreshold
	if threshold <= 0 {
		threshold = envInt64("S3PE_MULTIPART_THRESHOLD")
	}
	if threshold <= 0 {
		threshold = DefaultMultipartThreshold
	}
	partSize := cfg.PartSize
	if partSize <= 0 {
		partSize = envInt64("S3PE_PART_SIZE")
	}
	if partSize <= 0 {
		partSize = DefaultPartSize
	}
	if partSize < MinPartSize {
		partSize = MinPartSize
	}
	parallel := cfg.Parallel
	if parallel <= 0 {
		parallel = int(envInt64("S3PE_PARALLEL"))
	}
	if parallel <= 0 {
		parallel = DefaultParallel
	}
	hc := cfg.HTTPClient
	if hc == nil {
		hc = &http.Client{
			Transport:     http.DefaultTransport,
			CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
		}
	}
	logger := cfg.Logger
	if logger == nil {
		logger = log.New(os.Stderr, "", 0)
	}
	quiet := cfg.Quiet
	if q := os.Getenv("S3PE_QUIET"); q != "" && q != "0" {
		quiet = true
	}
	return &Client{
		endpoint:  endpoint,
		host:      host,
		region:    region,
		creds:     credentials{access, secret, token},
		bucket:    bucket,
		timeout:   timeout,
		maxTries:  tries,
		threshold: threshold,
		partSize:  partSize,
		parallel:  parallel,
		http:      hc,
		logger:    logger,
		quiet:     quiet,
	}, nil
}

// FromEnv builds a client configured entirely from the environment.
func FromEnv() (*Client, error) { return New(Config{}) }

// parseEndpoint returns the endpoint without trailing slash and the Host
// header value, with a default port dropped so it matches what net/http sends.
func parseEndpoint(raw string) (endpoint, host string, err error) {
	raw = strings.TrimRight(strings.TrimSpace(raw), "/")
	scheme, rest, ok := strings.Cut(raw, "://")
	if !ok {
		return "", "", configErr("endpoint must start with http:// or https://: %s", raw)
	}
	if scheme != "http" && scheme != "https" {
		return "", "", configErr("endpoint scheme must be http or https: %s", raw)
	}
	if rest == "" || strings.ContainsAny(rest, "/?#") {
		return "", "", configErr("endpoint must be scheme://host[:port], no path: %s", raw)
	}
	host = rest
	if h, port, ok := strings.Cut(rest, ":"); ok && !strings.Contains(h, ":") {
		if (scheme == "https" && port == "443") || (scheme == "http" && port == "80") {
			host = h
		}
	}
	return scheme + "://" + host, host, nil
}

// Endpoint is the endpoint, as scheme://host[:port].
func (c *Client) Endpoint() string { return c.endpoint }

// Bucket is the default bucket, empty if none.
func (c *Client) Bucket() string { return c.bucket }

// Resolve splits s3://bucket/key or applies the default bucket to a bare key.
func (c *Client) Resolve(key string) (bucket, objectKey string, err error) {
	if rest, ok := strings.CutPrefix(key, "s3://"); ok {
		b, k, _ := strings.Cut(rest, "/")
		if b == "" {
			return "", "", configErr("no bucket in URL: s3://%s", rest)
		}
		return b, k, nil
	}
	key = strings.TrimPrefix(key, "/")
	if c.bucket == "" {
		return "", "", configErr("no bucket: write the key as s3://bucket/%s, set S3PE_BUCKET or set Config.Bucket", key)
	}
	return c.bucket, key, nil
}

func (c *Client) resolveObject(key, verb string) (bucket, objectKey string, err error) {
	bucket, objectKey, err = c.Resolve(key)
	if err != nil {
		return "", "", err
	}
	if objectKey == "" {
		return "", "", configErr("no key to %s in s3://%s/", verb, bucket)
	}
	return bucket, objectKey, nil
}

func (c *Client) logf(format string, a ...any) {
	if !c.quiet {
		c.logger.Printf(format, a...)
	}
}

func (c *Client) warnf(format string, a ...any) {
	c.logger.Printf(format, a...)
}

// Read reads an object into memory.
func (c *Client) Read(ctx context.Context, key string) ([]byte, error) {
	bucket, k, err := c.resolveObject(key, "read")
	if err != nil {
		return nil, err
	}
	start := time.Now()
	resp, done, err := c.send(ctx, http.MethodGet, bucket, k, nil, nil, nil)
	if err != nil {
		return nil, err
	}
	defer done()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, transportErr(err, "reading s3://%s/%s", bucket, k)
	}
	sum := md5.Sum(data)
	if err := checkDownload(bucket, k, int64(len(data)), resp.ContentLength, etagOf(resp), hex.EncodeToString(sum[:])); err != nil {
		return nil, err
	}
	c.logf("↓ s3://%s/%s (%s, %.1f s)", bucket, k, FormatSize(int64(len(data))), time.Since(start).Seconds())
	return data, nil
}

// Write writes bytes to an object. An empty contentType is guessed from the
// key's extension. Bodies above the multipart threshold go up in parts.
func (c *Client) Write(ctx context.Context, key string, data []byte, contentType string) (Transfer, error) {
	return c.WriteWith(ctx, key, data, PutOptions{ContentType: contentType})
}

// WriteWith writes bytes to an object with metadata and headers (D17).
func (c *Client) WriteWith(ctx context.Context, key string, data []byte, opts PutOptions) (Transfer, error) {
	bucket, k, err := c.resolveObject(key, "write")
	if err != nil {
		return Transfer{}, err
	}
	start := time.Now()
	t, err := c.putSource(ctx, bucket, k, source{data: data, size: int64(len(data))}, opts, start)
	if err != nil {
		return Transfer{}, err
	}
	c.logf("↑ %s (%s, %.1f s)", t.URL(), FormatSize(t.Size), t.Elapsed.Seconds())
	return t, nil
}

// Upload streams a file to an object. A key that is empty or ends in "/"
// gets the file's base name appended. An empty contentType is guessed from
// the key. Files above the multipart threshold go up in parts.
func (c *Client) Upload(ctx context.Context, path, key, contentType string) (Transfer, error) {
	return c.UploadWith(ctx, path, key, PutOptions{ContentType: contentType})
}

// UploadWith streams a file to an object with metadata and headers (D17).
func (c *Client) UploadWith(ctx context.Context, path, key string, opts PutOptions) (Transfer, error) {
	bucket, k, err := c.Resolve(key)
	if err != nil {
		return Transfer{}, err
	}
	if k == "" || strings.HasSuffix(k, "/") {
		k += filepath.Base(path)
	}
	start := time.Now()
	info, err := os.Stat(path)
	if err != nil {
		return Transfer{}, ioErr(err, "reading", path)
	}
	t, err := c.putSource(ctx, bucket, k, source{path: path, size: info.Size()}, opts, start)
	if err != nil {
		return Transfer{}, err
	}
	c.logf("↑ %s → %s (%s, %.1f s)", path, t.URL(), FormatSize(t.Size), t.Elapsed.Seconds())
	return t, nil
}

// source is a body in memory or in a file.
type source struct {
	data []byte
	path string
	size int64
}

// part reads n bytes at offset.
func (s source) part(offset, n int64) ([]byte, error) {
	if s.path == "" {
		return s.data[offset : offset+n], nil
	}
	f, err := os.Open(s.path)
	if err != nil {
		return nil, ioErr(err, "opening", s.path)
	}
	defer f.Close()
	buf := make([]byte, n)
	if _, err := f.ReadAt(buf, offset); err != nil && err != io.EOF {
		return nil, ioErr(err, "reading", s.path)
	}
	return buf, nil
}

func (s source) payload() (*payload, error) {
	if s.path == "" {
		sum := md5.Sum(s.data)
		data := s.data
		return &payload{
			sha256: sha256Hex(data), md5: hex.EncodeToString(sum[:]), size: int64(len(data)),
			open: func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil },
		}, nil
	}
	return hashFile(s.path)
}

func putHeaders(key string, opts PutOptions) []pair {
	ct := opts.ContentType
	if ct == "" {
		ct = ContentTypeFor(key)
	}
	headers := []pair{{"content-type", ct}}
	names := make([]string, 0, len(opts.Metadata))
	for k := range opts.Metadata {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, k := range names {
		headers = append(headers, pair{"x-amz-meta-" + strings.ToLower(k), opts.Metadata[k]})
	}
	names = names[:0]
	for k := range opts.Headers {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, k := range names {
		headers = append(headers, pair{strings.ToLower(k), opts.Headers[k]})
	}
	return headers
}

func (c *Client) putSource(ctx context.Context, bucket, key string, src source, opts PutOptions, start time.Time) (Transfer, error) {
	headers := putHeaders(key, opts)
	if src.size > c.threshold {
		return c.multipart(ctx, bucket, key, src, headers, start)
	}
	body, err := src.payload()
	if err != nil {
		return Transfer{}, err
	}
	resp, done, err := c.send(ctx, http.MethodPut, bucket, key, nil, body, headers)
	if err != nil {
		return Transfer{}, err
	}
	done()
	etag := etagOf(resp)
	if err := checkUpload(bucket, key, body.md5, etag); err != nil {
		return Transfer{}, err
	}
	return Transfer{Bucket: bucket, Key: key, Size: src.size, ETag: etag, Elapsed: time.Since(start)}, nil
}

// MultipartETag is the ETag S3 gives a multipart upload:
// md5(concat(part MD5 bytes))-N, from the hex MD5s of the parts.
func MultipartETag(partMD5sHex []string) string {
	all := make([]byte, 0, 16*len(partMD5sHex))
	for _, h := range partMD5sHex {
		b, _ := hex.DecodeString(h)
		all = append(all, b...)
	}
	sum := md5.Sum(all)
	return hex.EncodeToString(sum[:]) + "-" + strconv.Itoa(len(partMD5sHex))
}

// ExpectedETag is the ETag the store would give a local file uploaded by
// this client: its MD5, or the multipart ETag (D15) when the file is larger
// than threshold, with parts of partSize.
func ExpectedETag(path string, threshold, partSize int64) (string, error) {
	info, err := os.Stat(path)
	if err != nil {
		return "", ioErr(err, "reading", path)
	}
	f, err := os.Open(path)
	if err != nil {
		return "", ioErr(err, "opening", path)
	}
	defer f.Close()
	if info.Size() <= threshold {
		h := md5.New()
		if _, err := io.Copy(h, f); err != nil {
			return "", ioErr(err, "reading", path)
		}
		return hex.EncodeToString(h.Sum(nil)), nil
	}
	if partSize < MinPartSize {
		partSize = MinPartSize
	}
	var md5s []string
	for remaining := info.Size(); remaining > 0; {
		n := remaining
		if n > partSize {
			n = partSize
		}
		h := md5.New()
		if _, err := io.CopyN(h, f, n); err != nil {
			return "", ioErr(err, "reading", path)
		}
		md5s = append(md5s, hex.EncodeToString(h.Sum(nil)))
		remaining -= n
	}
	return MultipartETag(md5s), nil
}

func (c *Client) multipart(ctx context.Context, bucket, key string, src source, headers []pair, start time.Time) (Transfer, error) {
	resp, done, err := c.send(ctx, http.MethodPost, bucket, key, []pair{{"uploads", ""}}, nil, headers)
	if err != nil {
		return Transfer{}, err
	}
	var init struct {
		UploadID string `xml:"UploadId"`
	}
	raw, _ := io.ReadAll(resp.Body)
	done()
	if xml.Unmarshal(raw, &init) != nil || init.UploadID == "" {
		return Transfer{}, transportErr(errors.New("no UploadId"), "initiating the multipart upload of s3://%s/%s", bucket, key)
	}
	uploadID := init.UploadID

	nParts := int((src.size + c.partSize - 1) / c.partSize)
	md5s := make([]string, nParts)
	errs := make([]error, nParts)
	indexes := make(chan int)
	var wg sync.WaitGroup
	var failed sync.Once
	stop := make(chan struct{})
	workers := c.parallel
	if workers > nParts {
		workers = nParts
	}
	for w := 0; w < workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range indexes {
				offset := int64(i) * c.partSize
				n := src.size - offset
				if n > c.partSize {
					n = c.partSize
				}
				sum, err := c.uploadPart(ctx, bucket, key, uploadID, i+1, src, offset, n)
				if err != nil {
					errs[i] = err
					failed.Do(func() { close(stop) })
					return
				}
				md5s[i] = sum
			}
		}()
	}
dispatch:
	for i := 0; i < nParts; i++ {
		select {
		case indexes <- i:
		case <-stop:
			break dispatch
		}
	}
	close(indexes)
	wg.Wait()

	for i := 0; i < nParts; i++ {
		if errs[i] != nil {
			c.abortMultipart(ctx, bucket, key, uploadID)
			return Transfer{}, errs[i]
		}
		if md5s[i] == "" {
			c.abortMultipart(ctx, bucket, key, uploadID)
			return Transfer{}, transportErr(errors.New("not uploaded"), "part %d of s3://%s/%s", i+1, bucket, key)
		}
	}

	var b strings.Builder
	b.WriteString("<CompleteMultipartUpload>")
	for i, sum := range md5s {
		fmt.Fprintf(&b, `<Part><PartNumber>%d</PartNumber><ETag>"%s"</ETag></Part>`, i+1, sum)
	}
	b.WriteString("</CompleteMultipartUpload>")
	body := bytesPayload([]byte(b.String()))
	resp, done, err = c.send(ctx, http.MethodPost, bucket, key, []pair{{"uploadId", uploadID}}, body, []pair{{"content-type", "application/xml"}})
	if err != nil {
		c.abortMultipart(ctx, bucket, key, uploadID)
		return Transfer{}, err
	}
	raw, _ = io.ReadAll(resp.Body)
	done()
	var e s3Error
	if xml.Unmarshal(raw, &e) == nil && e.XMLName.Local == "Error" && e.Code != "" {
		c.abortMultipart(ctx, bucket, key, uploadID)
		return Transfer{}, &Error{Status: 200, Code: e.Code, Message: e.Message, Bucket: bucket, Key: key, kind: ErrHTTP}
	}
	var result struct {
		ETag string `xml:"ETag"`
	}
	xml.Unmarshal(raw, &result)
	etag := strings.Trim(result.ETag, `"`)
	if want := MultipartETag(md5s); etag != "" && !strings.EqualFold(etag, want) {
		return Transfer{}, integrityErr("s3://%s/%s: multipart ETag %s differs from the expected %s", bucket, key, etag, want)
	}
	return Transfer{Bucket: bucket, Key: key, Size: src.size, ETag: etag, Elapsed: time.Since(start)}, nil
}

func (c *Client) uploadPart(ctx context.Context, bucket, key, uploadID string, number int, src source, offset, n int64) (string, error) {
	data, err := src.part(offset, n)
	if err != nil {
		return "", err
	}
	body := bytesPayload(data)
	query := []pair{{"partNumber", strconv.Itoa(number)}, {"uploadId", uploadID}}
	// An explicit content type: a web application firewall in front of the
	// store may parse an untyped body as form data and refuse it.
	resp, done, err := c.send(ctx, http.MethodPut, bucket, key, query, body, []pair{{"content-type", "application/octet-stream"}})
	if err != nil {
		return "", err
	}
	done()
	etag := etagOf(resp)
	if looksLikeMD5(etag) && !strings.EqualFold(etag, body.md5) {
		return "", integrityErr("s3://%s/%s part %d: ETag %s differs from the MD5 of what was sent (%s)", bucket, key, number, etag, body.md5)
	}
	return body.md5, nil
}

func (c *Client) abortMultipart(ctx context.Context, bucket, key, uploadID string) {
	// Abort with a fresh context: the caller's may already be cancelled.
	actx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 30*time.Second)
	defer cancel()
	_, done, err := c.send(actx, http.MethodDelete, bucket, key, []pair{{"uploadId", uploadID}}, nil, nil)
	if err != nil {
		c.warnf("could not abort the multipart upload of s3://%s/%s: %v", bucket, key, err)
		return
	}
	done()
	c.warnf("multipart upload of s3://%s/%s aborted", bucket, key)
}

func bytesPayload(data []byte) *payload {
	sum := md5.Sum(data)
	return &payload{
		sha256: sha256Hex(data), md5: hex.EncodeToString(sum[:]), size: int64(len(data)),
		open: func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil },
	}
}

// Download streams an object to a file. The bytes go to path + ".s3pe-part",
// renamed when complete, so a failure leaves nothing half-written at path.
// Missing parent directories are created.
func (c *Client) Download(ctx context.Context, key, path string) (Transfer, error) {
	bucket, k, err := c.resolveObject(key, "download")
	if err != nil {
		return Transfer{}, err
	}
	start := time.Now()
	resp, done, err := c.send(ctx, http.MethodGet, bucket, k, nil, nil, nil)
	if err != nil {
		return Transfer{}, err
	}
	defer done()
	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return Transfer{}, ioErr(err, "creating", dir)
		}
	}
	part := path + ".s3pe-part"
	f, err := os.Create(part)
	if err != nil {
		return Transfer{}, ioErr(err, "creating", part)
	}
	h := md5.New()
	n, copyErr := io.Copy(io.MultiWriter(f, h), resp.Body)
	closeErr := f.Close()
	fail := func(e error) (Transfer, error) {
		os.Remove(part)
		return Transfer{}, e
	}
	if copyErr != nil {
		return fail(transportErr(copyErr, "reading s3://%s/%s", bucket, k))
	}
	if closeErr != nil {
		return fail(ioErr(closeErr, "writing", part))
	}
	etag := etagOf(resp)
	if err := checkDownload(bucket, k, n, resp.ContentLength, etag, hex.EncodeToString(h.Sum(nil))); err != nil {
		return fail(err)
	}
	if err := os.Rename(part, path); err != nil {
		return fail(ioErr(err, "renaming to", path))
	}
	t := Transfer{Bucket: bucket, Key: k, Size: n, ETag: etag, Elapsed: time.Since(start)}
	c.logf("↓ %s → %s (%s, %.1f s)", t.URL(), path, FormatSize(n), t.Elapsed.Seconds())
	return t, nil
}

// List lists every object under a prefix (all pages). prefix may be an
// s3://bucket/prefix URL, a bare prefix, or "" for the whole bucket.
func (c *Client) List(ctx context.Context, prefix string) ([]Object, error) {
	l, err := c.ListWith(ctx, prefix, "")
	if err != nil {
		return nil, err
	}
	return l.Objects, nil
}

// ListDir lists one level: the objects directly under the prefix and the
// common prefixes ("folders") below it, with the delimiter "/" (D17).
func (c *Client) ListDir(ctx context.Context, prefix string) (Listing, error) {
	return c.ListWith(ctx, prefix, "/")
}

// ListWith lists with an optional delimiter, all pages.
func (c *Client) ListWith(ctx context.Context, prefix, delimiter string) (Listing, error) {
	bucket, p, err := c.Resolve(prefix)
	if err != nil {
		return Listing{}, err
	}
	start := time.Now()
	var out Listing
	token := ""
	for {
		query := []pair{{"list-type", "2"}}
		if p != "" {
			query = append(query, pair{"prefix", p})
		}
		if delimiter != "" {
			query = append(query, pair{"delimiter", delimiter})
		}
		if token != "" {
			query = append(query, pair{"continuation-token", token})
		}
		resp, done, err := c.send(ctx, http.MethodGet, bucket, "", query, nil, nil)
		if err != nil {
			return Listing{}, err
		}
		var page listing
		err = xml.NewDecoder(resp.Body).Decode(&page)
		done()
		if err != nil {
			return Listing{}, transportErr(err, "bad listing XML from s3://%s/%s", bucket, p)
		}
		for _, o := range page.Contents {
			out.Objects = append(out.Objects, Object{
				Key:          o.Key,
				Size:         o.Size,
				LastModified: parseTime(o.LastModified),
				ETag:         strings.Trim(o.ETag, `"`),
			})
		}
		for _, cp := range page.CommonPrefixes {
			if cp.Prefix != "" {
				out.Prefixes = append(out.Prefixes, cp.Prefix)
			}
		}
		if !page.IsTruncated || page.NextContinuationToken == "" {
			break
		}
		token = page.NextContinuationToken
	}
	plural := "s"
	if len(out.Objects) == 1 {
		plural = ""
	}
	c.logf("✔ %d object%s under s3://%s/%s (%.1f s)", len(out.Objects), plural, bucket, p, time.Since(start).Seconds())
	return out, nil
}

// Delete deletes an object. Deleting what does not exist is not an error.
func (c *Client) Delete(ctx context.Context, key string) error {
	bucket, k, err := c.resolveObject(key, "delete")
	if err != nil {
		return err
	}
	_, done, err := c.send(ctx, http.MethodDelete, bucket, k, nil, nil, nil)
	if err != nil {
		return err
	}
	done()
	c.logf("✖ s3://%s/%s", bucket, k)
	return nil
}

// DeleteMany deletes several objects with DeleteObjects, 1000 per request
// (D17). Keys may name different buckets. Any object the store could not
// delete is reported in one error of kind ErrHTTP with code DeleteErrors.
func (c *Client) DeleteMany(ctx context.Context, keys []string) error {
	byBucket := map[string][]string{}
	var buckets []string
	for _, key := range keys {
		b, k, err := c.resolveObject(key, "delete")
		if err != nil {
			return err
		}
		if _, seen := byBucket[b]; !seen {
			buckets = append(buckets, b)
		}
		byBucket[b] = append(byBucket[b], k)
	}
	var failed []string
	for _, bucket := range buckets {
		ks := byBucket[bucket]
		for i := 0; i < len(ks); i += 1000 {
			end := i + 1000
			if end > len(ks) {
				end = len(ks)
			}
			var b strings.Builder
			b.WriteString("<Delete><Quiet>true</Quiet>")
			for _, k := range ks[i:end] {
				b.WriteString("<Object><Key>")
				xml.EscapeText(&b, []byte(k))
				b.WriteString("</Key></Object>")
			}
			b.WriteString("</Delete>")
			body := bytesPayload([]byte(b.String()))
			sum := md5.Sum([]byte(b.String()))
			headers := []pair{
				{"content-type", "application/xml"},
				{"content-md5", base64.StdEncoding.EncodeToString(sum[:])},
			}
			resp, done, err := c.send(ctx, http.MethodPost, bucket, "", []pair{{"delete", ""}}, body, headers)
			if err != nil {
				return err
			}
			var result struct {
				Errors []struct {
					Key     string `xml:"Key"`
					Code    string `xml:"Code"`
					Message string `xml:"Message"`
				} `xml:"Error"`
			}
			err = xml.NewDecoder(resp.Body).Decode(&result)
			done()
			if err != nil && err != io.EOF {
				return transportErr(err, "bad DeleteResult XML from s3://%s/", bucket)
			}
			for _, e := range result.Errors {
				failed = append(failed, fmt.Sprintf("s3://%s/%s (%s: %s)", bucket, e.Key, e.Code, e.Message))
			}
		}
		plural := "s"
		if len(ks) == 1 {
			plural = ""
		}
		c.logf("✖ %d object%s in s3://%s/", len(ks), plural, bucket)
	}
	if len(failed) > 0 {
		return &Error{Status: 200, Code: "DeleteErrors", Message: fmt.Sprintf("%d not deleted: %s", len(failed), strings.Join(failed, ", ")), Bucket: buckets[0], kind: ErrHTTP}
	}
	return nil
}

// Copy copies an object inside the store (CopyObject, D17). Both keys may
// be s3:// URLs; the copy keeps the source's metadata.
func (c *Client) Copy(ctx context.Context, from, to string) (Transfer, error) {
	sb, sk, err := c.resolveObject(from, "copy")
	if err != nil {
		return Transfer{}, err
	}
	bucket, k, err := c.resolveObject(to, "copy to")
	if err != nil {
		return Transfer{}, err
	}
	start := time.Now()
	headers := []pair{{"x-amz-copy-source", "/" + uriEncode(sb, false) + "/" + uriEncode(sk, true)}}
	resp, done, err := c.send(ctx, http.MethodPut, bucket, k, nil, nil, headers)
	if err != nil {
		return Transfer{}, err
	}
	raw, _ := io.ReadAll(resp.Body)
	done()
	var e s3Error
	if xml.Unmarshal(raw, &e) == nil && e.XMLName.Local == "Error" && e.Code != "" {
		return Transfer{}, &Error{Status: 200, Code: e.Code, Message: e.Message, Bucket: bucket, Key: k, kind: ErrHTTP}
	}
	var result struct {
		ETag string `xml:"ETag"`
	}
	xml.Unmarshal(raw, &result)
	var size int64
	if o, err := c.Head(ctx, "s3://"+bucket+"/"+k); err == nil && o != nil {
		size = o.Size
	}
	t := Transfer{Bucket: bucket, Key: k, Size: size, ETag: strings.Trim(result.ETag, `"`), Elapsed: time.Since(start)}
	c.logf("⧉ s3://%s/%s → %s (%s, %.1f s)", sb, sk, t.URL(), FormatSize(size), t.Elapsed.Seconds())
	return t, nil
}

// Head returns the metadata of an object, or nil, nil when it does not exist.
func (c *Client) Head(ctx context.Context, key string) (*Object, error) {
	bucket, k, err := c.resolveObject(key, "inspect")
	if err != nil {
		return nil, err
	}
	resp, done, err := c.send(ctx, http.MethodHead, bucket, k, nil, nil, nil)
	if err != nil {
		if errors.Is(err, ErrNotFound) {
			return nil, nil
		}
		return nil, err
	}
	done()
	size := resp.ContentLength
	if size < 0 {
		size = 0
	}
	meta := map[string]string{}
	for name, values := range resp.Header {
		if short, ok := strings.CutPrefix(strings.ToLower(name), "x-amz-meta-"); ok && len(values) > 0 {
			meta[short] = values[0]
		}
	}
	return &Object{
		Key:          k,
		Size:         size,
		LastModified: parseTime(resp.Header.Get("Last-Modified")),
		ETag:         etagOf(resp),
		ContentType:  resp.Header.Get("Content-Type"),
		Metadata:     meta,
	}, nil
}

// Exists reports whether an object exists.
func (c *Client) Exists(ctx context.Context, key string) (bool, error) {
	o, err := c.Head(ctx, key)
	return o != nil, err
}

// Buckets lists the buckets the credentials can see.
func (c *Client) Buckets(ctx context.Context) ([]Bucket, error) {
	resp, done, err := c.send(ctx, http.MethodGet, "", "", nil, nil, nil)
	if err != nil {
		return nil, err
	}
	defer done()
	var list bucketList
	if err := xml.NewDecoder(resp.Body).Decode(&list); err != nil {
		return nil, transportErr(err, "bad bucket list XML")
	}
	out := make([]Bucket, len(list.Buckets))
	for i, b := range list.Buckets {
		out[i] = Bucket{Name: b.Name, Created: parseTime(b.CreationDate)}
	}
	return out, nil
}

// Presign returns a URL that reads (GET) or writes (PUT) the object without
// credentials for expires, at most seven days (D16).
func (c *Client) Presign(key, method string, expires time.Duration) (string, error) {
	bucket, k, err := c.resolveObject(key, "pre-sign")
	if err != nil {
		return "", err
	}
	method = strings.ToUpper(method)
	if method != http.MethodGet && method != http.MethodPut {
		return "", configErr("presign: method must be GET or PUT, not %s", method)
	}
	secs := int64(expires / time.Second)
	if secs < 1 {
		secs = 1
	}
	if secs > MaxPresignSecs {
		secs = MaxPresignSecs
	}
	path := "/" + uriEncode(bucket, false) + "/" + uriEncode(k, true)
	qs := presign(presignRequest{
		method:  method,
		host:    c.host,
		path:    path,
		amzDate: amzDate(time.Now()),
		expires: secs,
	}, c.creds, c.region)
	return c.endpoint + path + "?" + qs, nil
}

// SyncUp uploads every file under dir whose remote ETag differs from the
// local one or which is missing remotely; see SyncOptions (D18).
func (c *Client) SyncUp(ctx context.Context, dir, prefix string, opts SyncOptions) (SyncReport, error) {
	bucket, p, err := c.Resolve(prefix)
	if err != nil {
		return SyncReport{}, err
	}
	p = normalizePrefix(p)
	start := time.Now()
	local, err := walk(dir)
	if err != nil {
		return SyncReport{}, err
	}
	remote := map[string]Object{}
	objects, err := c.List(ctx, "s3://"+bucket+"/"+p)
	if err != nil {
		return SyncReport{}, err
	}
	for _, o := range objects {
		if rel, ok := strings.CutPrefix(o.Key, p); ok {
			remote[rel] = o
		}
	}
	var report SyncReport
	queue := make([]string, 0, len(local))
	for _, rel := range local {
		if rel != opts.Last {
			queue = append(queue, rel)
		}
	}
	if opts.Last != "" {
		if !contains(local, opts.Last) {
			return SyncReport{}, configErr("last = %s: no such file under %s", opts.Last, dir)
		}
		queue = append(queue, opts.Last)
	}
	for _, rel := range queue {
		path := filepath.Join(dir, filepath.FromSlash(rel))
		same := false
		if o, ok := remote[rel]; ok {
			info, err := os.Stat(path)
			if err != nil {
				return report, ioErr(err, "reading", path)
			}
			if info.Size() == o.Size {
				etag, err := ExpectedETag(path, c.threshold, c.partSize)
				if err != nil {
					return report, err
				}
				same = strings.EqualFold(etag, o.ETag)
			}
		}
		if same {
			report.Skipped = append(report.Skipped, rel)
			continue
		}
		if _, err := c.Upload(ctx, path, "s3://"+bucket+"/"+p+rel, ""); err != nil {
			return report, err
		}
		report.Transferred = append(report.Transferred, rel)
	}
	if opts.Delete {
		var gone, urls []string
		for rel := range remote {
			if !contains(local, rel) {
				gone = append(gone, rel)
				urls = append(urls, "s3://"+bucket+"/"+p+rel)
			}
		}
		sort.Strings(gone)
		if len(urls) > 0 {
			if err := c.DeleteMany(ctx, urls); err != nil {
				return report, err
			}
			report.Deleted = gone
		}
	}
	report.Elapsed = time.Since(start)
	c.logf("✔ sync %s → s3://%s/%s: %d uploaded, %d unchanged, %d deleted (%.1f s)", dir, bucket, p, len(report.Transferred), len(report.Skipped), len(report.Deleted), report.Elapsed.Seconds())
	return report, nil
}

// SyncDown downloads every object under prefix whose local copy is missing
// or differs; with Delete, removes local files that are not remote (D18).
func (c *Client) SyncDown(ctx context.Context, prefix, dir string, opts SyncOptions) (SyncReport, error) {
	bucket, p, err := c.Resolve(prefix)
	if err != nil {
		return SyncReport{}, err
	}
	p = normalizePrefix(p)
	start := time.Now()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return SyncReport{}, ioErr(err, "creating", dir)
	}
	objects, err := c.List(ctx, "s3://"+bucket+"/"+p)
	if err != nil {
		return SyncReport{}, err
	}
	var report SyncReport
	remote := map[string]bool{}
	for _, o := range objects {
		rel, ok := strings.CutPrefix(o.Key, p)
		if !ok || rel == "" || strings.HasSuffix(rel, "/") {
			continue
		}
		remote[rel] = true
		path := filepath.Join(dir, filepath.FromSlash(rel))
		same := false
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() && info.Size() == o.Size {
			etag, err := ExpectedETag(path, c.threshold, c.partSize)
			if err != nil {
				return report, err
			}
			same = strings.EqualFold(etag, o.ETag)
		}
		if same {
			report.Skipped = append(report.Skipped, rel)
			continue
		}
		if _, err := c.Download(ctx, "s3://"+bucket+"/"+o.Key, path); err != nil {
			return report, err
		}
		report.Transferred = append(report.Transferred, rel)
	}
	if opts.Delete {
		local, err := walk(dir)
		if err != nil {
			return report, err
		}
		for _, rel := range local {
			if !remote[rel] {
				if err := os.Remove(filepath.Join(dir, filepath.FromSlash(rel))); err != nil {
					return report, ioErr(err, "removing", rel)
				}
				report.Deleted = append(report.Deleted, rel)
			}
		}
	}
	report.Elapsed = time.Since(start)
	c.logf("✔ sync s3://%s/%s → %s: %d downloaded, %d unchanged, %d deleted (%.1f s)", bucket, p, dir, len(report.Transferred), len(report.Skipped), len(report.Deleted), report.Elapsed.Seconds())
	return report, nil
}

// normalizePrefix returns a prefix that is empty or ends with "/".
func normalizePrefix(p string) string {
	if p == "" || strings.HasSuffix(p, "/") {
		return p
	}
	return p + "/"
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// walk lists the relative paths (with "/") of every regular file under dir,
// sorted, skipping .s3pe-part files and .DS_Store.
func walk(dir string) ([]string, error) {
	var out []string
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return err
		}
		rel = filepath.ToSlash(rel)
		if strings.HasSuffix(rel, ".s3pe-part") || d.Name() == ".DS_Store" || !d.Type().IsRegular() {
			return nil
		}
		out = append(out, rel)
		return nil
	})
	if err != nil {
		return nil, ioErr(err, "walking", dir)
	}
	sort.Strings(out)
	return out, nil
}

// payload is a request body that can be reopened for each attempt.
type payload struct {
	sha256, md5 string
	size        int64
	open        func() (io.ReadCloser, error)
}

func hashFile(path string) (*payload, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, ioErr(err, "opening", path)
	}
	defer f.Close()
	sha := sha256New()
	m := md5.New()
	n, err := io.Copy(io.MultiWriter(sha, m), f)
	if err != nil {
		return nil, ioErr(err, "reading", path)
	}
	return &payload{
		sha256: hex.EncodeToString(sha.Sum(nil)),
		md5:    hex.EncodeToString(m.Sum(nil)),
		size:   n,
		open: func() (io.ReadCloser, error) {
			return os.Open(path)
		},
	}, nil
}

var transient = map[int]bool{429: true, 500: true, 502: true, 503: true, 504: true}

func backoff(try int) time.Duration {
	shift := try - 1
	if shift > 4 {
		shift = 4
	}
	return time.Duration(1<<shift) * time.Second
}

// send performs one S3 request with signing, retries and error mapping.
// headers are request headers: x-amz-* ones are signed, the rest are not.
// A successful response is returned with its body open; done closes the
// body and releases the call's timeout, and must be called exactly once.
func (c *Client) send(ctx context.Context, method, bucket, key string, query []pair, body *payload, headers []pair) (resp *http.Response, done func(), err error) {
	cancel := context.CancelFunc(func() {})
	if _, hasDeadline := ctx.Deadline(); !hasDeadline {
		ctx, cancel = context.WithTimeout(ctx, c.timeout)
	}
	resp, err = c.sendRetrying(ctx, method, bucket, key, query, body, headers)
	if err != nil {
		cancel()
		return nil, nil, err
	}
	return resp, func() { resp.Body.Close(); cancel() }, nil
}

func (c *Client) sendRetrying(ctx context.Context, method, bucket, key string, query []pair, body *payload, headers []pair) (*http.Response, error) {
	path := "/"
	switch {
	case bucket == "":
	case key == "":
		path = "/" + uriEncode(bucket, false)
	default:
		path = "/" + uriEncode(bucket, false) + "/" + uriEncode(key, true)
	}
	rawURL := c.endpoint + path
	if qs := canonicalQuery(query); qs != "" {
		rawURL += "?" + qs
	}
	payloadHash, size := emptySHA256, int64(0)
	if body != nil {
		payloadHash, size = body.sha256, body.size
	}

	for try := 1; ; try++ {
		resp, err := c.attempt(ctx, method, rawURL, path, query, payloadHash, size, body, headers)
		if err != nil {
			if try < c.maxTries && ctx.Err() == nil {
				wait := backoff(try)
				c.warnf("%s; retrying in %d s (%d/%d)", err.Error(), int(wait.Seconds()), try, c.maxTries)
				if sleep(ctx, wait) {
					continue
				}
			}
			return nil, transportErr(err, "%s %s", method, rawURL)
		}
		if transient[resp.StatusCode] && try < c.maxTries && ctx.Err() == nil {
			resp.Body.Close()
			wait := backoff(try)
			c.warnf("HTTP %d from %s s3://%s/%s; retrying in %d s (%d/%d)", resp.StatusCode, method, bucket, key, int(wait.Seconds()), try, c.maxTries)
			if sleep(ctx, wait) {
				continue
			}
		}
		return c.check(resp, method, bucket, key)
	}
}

func sleep(ctx context.Context, d time.Duration) bool {
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-t.C:
		return true
	case <-ctx.Done():
		return false
	}
}

func (c *Client) attempt(ctx context.Context, method, rawURL, path string, query []pair, payloadHash string, size int64, body *payload, headers []pair) (*http.Response, error) {
	var rc io.ReadCloser
	if body != nil {
		var err error
		rc, err = body.open()
		if err != nil {
			return nil, err
		}
	}
	req, err := http.NewRequestWithContext(ctx, method, rawURL, rc)
	if err != nil {
		if rc != nil {
			rc.Close()
		}
		return nil, err
	}
	if body != nil || method == http.MethodPut || method == http.MethodPost {
		req.ContentLength = size
		if size == 0 {
			req.Body = http.NoBody
		}
	}
	req.Header.Set("User-Agent", userAgent)
	var signed []pair
	for _, h := range headers {
		if strings.HasPrefix(h.name, "x-amz-") {
			signed = append(signed, h)
		} else {
			req.Header.Set(h.name, h.value)
		}
	}
	for _, h := range sign(signRequest{
		method:       method,
		host:         c.host,
		path:         path,
		query:        query,
		payloadHash:  payloadHash,
		amzDate:      amzDate(time.Now()),
		extraHeaders: signed,
	}, c.creds, c.region) {
		req.Header.Set(h.name, h.value)
	}
	return c.http.Do(req)
}

func (c *Client) check(resp *http.Response, method, bucket, key string) (*http.Response, error) {
	if resp.StatusCode >= 200 && resp.StatusCode < 300 {
		return resp, nil
	}
	defer resp.Body.Close()
	var code, message string
	if method != http.MethodHead {
		raw, _ := io.ReadAll(io.LimitReader(resp.Body, 64*1024))
		var e s3Error
		if xml.Unmarshal(raw, &e) == nil {
			code, message = e.Code, e.Message
		}
	}
	err := &Error{Status: resp.StatusCode, Code: code, Message: message, Bucket: bucket, Key: key, kind: ErrHTTP}
	switch resp.StatusCode {
	case http.StatusNotFound:
		err.kind = ErrNotFound
	case http.StatusForbidden:
		err.kind = ErrAccessDenied
	}
	return nil, err
}

type listing struct {
	Contents []struct {
		Key          string `xml:"Key"`
		Size         int64  `xml:"Size"`
		LastModified string `xml:"LastModified"`
		ETag         string `xml:"ETag"`
	} `xml:"Contents"`
	CommonPrefixes []struct {
		Prefix string `xml:"Prefix"`
	} `xml:"CommonPrefixes"`
	IsTruncated           bool   `xml:"IsTruncated"`
	NextContinuationToken string `xml:"NextContinuationToken"`
}

type bucketList struct {
	Buckets []struct {
		Name         string `xml:"Name"`
		CreationDate string `xml:"CreationDate"`
	} `xml:"Buckets>Bucket"`
}

type s3Error struct {
	XMLName xml.Name
	Code    string `xml:"Code"`
	Message string `xml:"Message"`
}

func parseTime(s string) time.Time {
	s = strings.TrimSpace(s)
	for _, layout := range []string{time.RFC3339Nano, time.RFC3339, http.TimeFormat, time.RFC1123, time.RFC1123Z} {
		if t, err := time.Parse(layout, s); err == nil {
			return t
		}
	}
	return time.Time{}
}

func etagOf(resp *http.Response) string {
	return strings.Trim(resp.Header.Get("ETag"), `"`)
}

func looksLikeMD5(etag string) bool {
	if len(etag) != 32 {
		return false
	}
	_, err := hex.DecodeString(etag)
	return err == nil
}

func checkUpload(bucket, key, sum, etag string) error {
	if looksLikeMD5(etag) && !strings.EqualFold(etag, sum) {
		return integrityErr("s3://%s/%s: the server's ETag %s differs from the MD5 of what was sent (%s)", bucket, key, etag, sum)
	}
	return nil
}

func checkDownload(bucket, key string, got, expected int64, etag, sum string) error {
	if expected >= 0 && expected != got {
		return integrityErr("s3://%s/%s: received %d bytes, Content-Length said %d", bucket, key, got, expected)
	}
	if looksLikeMD5(etag) && !strings.EqualFold(etag, sum) {
		return integrityErr("s3://%s/%s: MD5 of what arrived (%s) differs from the ETag %s", bucket, key, sum, etag)
	}
	return nil
}

// FormatSize renders a byte count with SI units and one decimal:
// "12.3 MB", "1.2 kB", "512 B".
func FormatSize(n int64) string {
	units := []string{"B", "kB", "MB", "GB", "TB"}
	v := float64(n)
	i := 0
	for v >= 1000 && i < len(units)-1 {
		v /= 1000
		i++
	}
	if i == 0 {
		return strconv.FormatInt(n, 10) + " B"
	}
	return fmt.Sprintf("%.1f %s", v, units[i])
}

// ContentTypeFor is the content type a key's extension suggests
// (application/octet-stream when unknown). Every edition of s3pe uses the
// same table.
func ContentTypeFor(key string) string {
	name := strings.ToLower(key)
	if i := strings.LastIndex(name, "/"); i >= 0 {
		name = name[i+1:]
	}
	for _, s := range []string{".csv.gz", ".tsv.gz", ".json.gz"} {
		if strings.HasSuffix(name, s) {
			return "application/gzip"
		}
	}
	ext := ""
	if i := strings.LastIndex(name, "."); i >= 0 {
		ext = name[i+1:]
	}
	switch ext {
	case "csv":
		return "text/csv"
	case "tsv":
		return "text/tab-separated-values"
	case "json":
		return "application/json"
	case "geojson":
		return "application/geo+json"
	case "parquet":
		return "application/vnd.apache.parquet"
	case "gz":
		return "application/gzip"
	case "zip":
		return "application/zip"
	case "txt":
		return "text/plain"
	case "md":
		return "text/markdown"
	case "html", "htm":
		return "text/html"
	case "xml":
		return "application/xml"
	case "svg":
		return "image/svg+xml"
	case "png":
		return "image/png"
	case "jpg", "jpeg":
		return "image/jpeg"
	case "gif":
		return "image/gif"
	case "webp":
		return "image/webp"
	case "pdf":
		return "application/pdf"
	case "xlsx":
		return "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
	case "docx":
		return "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
	case "js":
		return "text/javascript"
	case "css":
		return "text/css"
	case "yaml", "yml":
		return "application/yaml"
	case "toml":
		return "application/toml"
	}
	return "application/octet-stream"
}

// The package-level verbs share one client, built from the environment on
// first use.
var (
	defaultOnce   sync.Once
	defaultClient *Client
	defaultErr    error
)

func defaultC() (*Client, error) {
	defaultOnce.Do(func() { defaultClient, defaultErr = FromEnv() })
	return defaultClient, defaultErr
}

// ReadS3 reads an object into memory, with a client built from the environment.
func ReadS3(ctx context.Context, key string) ([]byte, error) {
	c, err := defaultC()
	if err != nil {
		return nil, err
	}
	return c.Read(ctx, key)
}

// WriteS3 writes bytes to an object, with a client built from the environment.
func WriteS3(ctx context.Context, key string, data []byte, contentType string) (Transfer, error) {
	c, err := defaultC()
	if err != nil {
		return Transfer{}, err
	}
	return c.Write(ctx, key, data, contentType)
}

// DownloadS3 downloads an object to a file, with a client built from the environment.
func DownloadS3(ctx context.Context, key, path string) (Transfer, error) {
	c, err := defaultC()
	if err != nil {
		return Transfer{}, err
	}
	return c.Download(ctx, key, path)
}

// UploadS3 uploads a file, with a client built from the environment.
func UploadS3(ctx context.Context, path, key, contentType string) (Transfer, error) {
	c, err := defaultC()
	if err != nil {
		return Transfer{}, err
	}
	return c.Upload(ctx, path, key, contentType)
}

// ListS3 lists the objects under a prefix, with a client built from the environment.
func ListS3(ctx context.Context, prefix string) ([]Object, error) {
	c, err := defaultC()
	if err != nil {
		return nil, err
	}
	return c.List(ctx, prefix)
}

// ListDirS3 lists one level of a prefix, with a client built from the environment.
func ListDirS3(ctx context.Context, prefix string) (Listing, error) {
	c, err := defaultC()
	if err != nil {
		return Listing{}, err
	}
	return c.ListDir(ctx, prefix)
}

// DeleteS3 deletes an object, with a client built from the environment.
func DeleteS3(ctx context.Context, key string) error {
	c, err := defaultC()
	if err != nil {
		return err
	}
	return c.Delete(ctx, key)
}

// DeleteManyS3 deletes several objects in batches, with a client built from the environment.
func DeleteManyS3(ctx context.Context, keys []string) error {
	c, err := defaultC()
	if err != nil {
		return err
	}
	return c.DeleteMany(ctx, keys)
}

// CopyS3 copies an object inside the store, with a client built from the environment.
func CopyS3(ctx context.Context, from, to string) (Transfer, error) {
	c, err := defaultC()
	if err != nil {
		return Transfer{}, err
	}
	return c.Copy(ctx, from, to)
}

// ExistsS3 reports whether an object exists, with a client built from the environment.
func ExistsS3(ctx context.Context, key string) (bool, error) {
	c, err := defaultC()
	if err != nil {
		return false, err
	}
	return c.Exists(ctx, key)
}

// HeadS3 returns an object's metadata (nil if absent), with a client built from the environment.
func HeadS3(ctx context.Context, key string) (*Object, error) {
	c, err := defaultC()
	if err != nil {
		return nil, err
	}
	return c.Head(ctx, key)
}

// BucketsS3 lists the buckets the credentials can see, with a client built from the environment.
func BucketsS3(ctx context.Context) ([]Bucket, error) {
	c, err := defaultC()
	if err != nil {
		return nil, err
	}
	return c.Buckets(ctx)
}

// PresignS3 returns a pre-signed URL, with a client built from the environment.
func PresignS3(key, method string, expires time.Duration) (string, error) {
	c, err := defaultC()
	if err != nil {
		return "", err
	}
	return c.Presign(key, method, expires)
}

// SyncUpS3 uploads a directory to a prefix, with a client built from the environment.
func SyncUpS3(ctx context.Context, dir, prefix string, opts SyncOptions) (SyncReport, error) {
	c, err := defaultC()
	if err != nil {
		return SyncReport{}, err
	}
	return c.SyncUp(ctx, dir, prefix, opts)
}

// SyncDownS3 downloads a prefix to a directory, with a client built from the environment.
func SyncDownS3(ctx context.Context, prefix, dir string, opts SyncOptions) (SyncReport, error) {
	c, err := defaultC()
	if err != nil {
		return SyncReport{}, err
	}
	return c.SyncDown(ctx, prefix, dir, opts)
}

func sha256New() hash.Hash { return sha256.New() }
