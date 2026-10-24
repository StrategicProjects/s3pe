package s3pe

import (
	"bytes"
	"context"
	"crypto/md5"
	"crypto/sha256"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"fmt"
	"hash"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	// DefaultRegion is the signing region of that server.
	DefaultRegion = "us-east-1"
	// DefaultTimeout is the time one call may take before it is abandoned.
	DefaultTimeout = 300 * time.Second
	// DefaultMaxTries is the number of attempts per call: the first and two retries.
	DefaultMaxTries = 3

	version   = "0.2.0"
	userAgent = "s3pe-go/" + version
)

// Sentinel errors, to be tested with errors.Is.
var (
	// ErrConfig: missing credentials, missing bucket, bad endpoint.
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
}

// Bucket is a bucket as listed by the store.
type Bucket struct {
	Name    string
	Created time.Time
}

// Transfer is what a completed upload or download reports.
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

// Config configures a Client. Zero fields are taken from the environment
// (S3PE_ENDPOINT or AWS_ENDPOINT_URL, AWS_REGION or AWS_DEFAULT_REGION,
// AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN, S3PE_BUCKET,
// S3PE_QUIET) and then from the defaults.
type Config struct {
	// Endpoint is scheme://host[:port], no path.
	Endpoint string
	// Region is the signing region (default us-east-1).
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
	endpoint string
	host     string
	region   string
	creds    credentials
	bucket   string
	timeout  time.Duration
	maxTries int
	http     *http.Client
	logger   *log.Logger
	quiet    bool
}

func env(names ...string) string {
	for _, n := range names {
		if v := strings.TrimSpace(os.Getenv(n)); v != "" {
			return v
		}
	}
	return ""
}

// New builds a client from cfg, the environment and the defaults. It fails
// without credentials or with an endpoint that is not scheme://host[:port].
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
		endpoint: endpoint,
		host:     host,
		region:   region,
		creds:    credentials{access, secret, token},
		bucket:   bucket,
		timeout:  timeout,
		maxTries: tries,
		http:     hc,
		logger:   logger,
		quiet:    quiet,
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
	resp, done, err := c.send(ctx, http.MethodGet, bucket, k, nil, nil, "")
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
// key's extension. The server's ETag is checked against the local MD5.
func (c *Client) Write(ctx context.Context, key string, data []byte, contentType string) (Transfer, error) {
	bucket, k, err := c.resolveObject(key, "write")
	if err != nil {
		return Transfer{}, err
	}
	if contentType == "" {
		contentType = ContentTypeFor(k)
	}
	start := time.Now()
	sum := md5.Sum(data)
	body := &payload{
		sha256: sha256Hex(data), md5: hex.EncodeToString(sum[:]), size: int64(len(data)),
		open: func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(data)), nil },
	}
	resp, done, err := c.send(ctx, http.MethodPut, bucket, k, nil, body, contentType)
	if err != nil {
		return Transfer{}, err
	}
	done()
	etag := etagOf(resp)
	if err := checkUpload(bucket, k, body.md5, etag); err != nil {
		return Transfer{}, err
	}
	t := Transfer{Bucket: bucket, Key: k, Size: body.size, ETag: etag, Elapsed: time.Since(start)}
	c.logf("↑ %s (%s, %.1f s)", t.URL(), FormatSize(t.Size), t.Elapsed.Seconds())
	return t, nil
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
	resp, done, err := c.send(ctx, http.MethodGet, bucket, k, nil, nil, "")
	if err != nil {
		return Transfer{}, err
	}
	defer done()
	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return Transfer{}, &Error{kind: ErrTransport, Message: "creating " + dir + ": " + err.Error(), err: err}
		}
	}
	part := path + ".s3pe-part"
	f, err := os.Create(part)
	if err != nil {
		return Transfer{}, &Error{kind: ErrTransport, Message: "creating " + part + ": " + err.Error(), err: err}
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
		return fail(&Error{kind: ErrTransport, Message: "writing " + part + ": " + closeErr.Error(), err: closeErr})
	}
	etag := etagOf(resp)
	if err := checkDownload(bucket, k, n, resp.ContentLength, etag, hex.EncodeToString(h.Sum(nil))); err != nil {
		return fail(err)
	}
	if err := os.Rename(part, path); err != nil {
		return fail(&Error{kind: ErrTransport, Message: "renaming to " + path + ": " + err.Error(), err: err})
	}
	t := Transfer{Bucket: bucket, Key: k, Size: n, ETag: etag, Elapsed: time.Since(start)}
	c.logf("↓ %s → %s (%s, %.1f s)", t.URL(), path, FormatSize(n), t.Elapsed.Seconds())
	return t, nil
}

// Upload streams a file to an object (two passes over the file: one to
// hash, one to send). A key that is empty or ends in "/" gets the file's
// base name appended. An empty contentType is guessed from the key.
func (c *Client) Upload(ctx context.Context, path, key, contentType string) (Transfer, error) {
	bucket, k, err := c.Resolve(key)
	if err != nil {
		return Transfer{}, err
	}
	if k == "" || strings.HasSuffix(k, "/") {
		k += filepath.Base(path)
	}
	if contentType == "" {
		contentType = ContentTypeFor(k)
	}
	start := time.Now()
	body, err := hashFile(path)
	if err != nil {
		return Transfer{}, err
	}
	resp, done, err := c.send(ctx, http.MethodPut, bucket, k, nil, body, contentType)
	if err != nil {
		return Transfer{}, err
	}
	done()
	etag := etagOf(resp)
	if err := checkUpload(bucket, k, body.md5, etag); err != nil {
		return Transfer{}, err
	}
	t := Transfer{Bucket: bucket, Key: k, Size: body.size, ETag: etag, Elapsed: time.Since(start)}
	c.logf("↑ %s → %s (%s, %.1f s)", path, t.URL(), FormatSize(t.Size), t.Elapsed.Seconds())
	return t, nil
}

// List lists every object under a prefix (all pages). prefix may be an
// s3://bucket/prefix URL, a bare prefix, or "" for the whole bucket.
func (c *Client) List(ctx context.Context, prefix string) ([]Object, error) {
	bucket, p, err := c.Resolve(prefix)
	if err != nil {
		return nil, err
	}
	start := time.Now()
	var objects []Object
	token := ""
	for {
		query := []pair{{"list-type", "2"}}
		if p != "" {
			query = append(query, pair{"prefix", p})
		}
		if token != "" {
			query = append(query, pair{"continuation-token", token})
		}
		resp, done, err := c.send(ctx, http.MethodGet, bucket, "", query, nil, "")
		if err != nil {
			return nil, err
		}
		var page listing
		err = xml.NewDecoder(resp.Body).Decode(&page)
		done()
		if err != nil {
			return nil, transportErr(err, "bad listing XML from s3://%s/%s", bucket, p)
		}
		for _, o := range page.Contents {
			objects = append(objects, Object{
				Key:          o.Key,
				Size:         o.Size,
				LastModified: parseTime(o.LastModified),
				ETag:         strings.Trim(o.ETag, `"`),
			})
		}
		if !page.IsTruncated || page.NextContinuationToken == "" {
			break
		}
		token = page.NextContinuationToken
	}
	plural := "s"
	if len(objects) == 1 {
		plural = ""
	}
	c.logf("✔ %d object%s under s3://%s/%s (%.1f s)", len(objects), plural, bucket, p, time.Since(start).Seconds())
	return objects, nil
}

// Delete deletes an object. Deleting what does not exist is not an error.
func (c *Client) Delete(ctx context.Context, key string) error {
	bucket, k, err := c.resolveObject(key, "delete")
	if err != nil {
		return err
	}
	_, done, err := c.send(ctx, http.MethodDelete, bucket, k, nil, nil, "")
	if err != nil {
		return err
	}
	done()
	c.logf("✖ s3://%s/%s", bucket, k)
	return nil
}

// Head returns the metadata of an object, or nil, nil when it does not exist.
func (c *Client) Head(ctx context.Context, key string) (*Object, error) {
	bucket, k, err := c.resolveObject(key, "inspect")
	if err != nil {
		return nil, err
	}
	resp, done, err := c.send(ctx, http.MethodHead, bucket, k, nil, nil, "")
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
	return &Object{
		Key:          k,
		Size:         size,
		LastModified: parseTime(resp.Header.Get("Last-Modified")),
		ETag:         etagOf(resp),
		ContentType:  resp.Header.Get("Content-Type"),
	}, nil
}

// Exists reports whether an object exists.
func (c *Client) Exists(ctx context.Context, key string) (bool, error) {
	o, err := c.Head(ctx, key)
	return o != nil, err
}

// Buckets lists the buckets the credentials can see.
func (c *Client) Buckets(ctx context.Context) ([]Bucket, error) {
	resp, done, err := c.send(ctx, http.MethodGet, "", "", nil, nil, "")
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

// payload is a request body that can be reopened for each attempt.
type payload struct {
	sha256, md5 string
	size        int64
	open        func() (io.ReadCloser, error)
}

func hashFile(path string) (*payload, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, &Error{kind: ErrTransport, Message: "opening " + path + ": " + err.Error(), err: err}
	}
	defer f.Close()
	sha := sha256New()
	m := md5.New()
	n, err := io.Copy(io.MultiWriter(sha, m), f)
	if err != nil {
		return nil, &Error{kind: ErrTransport, Message: "reading " + path + ": " + err.Error(), err: err}
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

// send performs one S3 request with signing, retries and error mapping. A
// successful response is returned with its body open; done closes the body
// and releases the call's timeout, and must be called exactly once.
func (c *Client) send(ctx context.Context, method, bucket, key string, query []pair, body *payload, contentType string) (resp *http.Response, done func(), err error) {
	cancel := context.CancelFunc(func() {})
	if _, hasDeadline := ctx.Deadline(); !hasDeadline {
		ctx, cancel = context.WithTimeout(ctx, c.timeout)
	}
	resp, err = c.sendRetrying(ctx, method, bucket, key, query, body, contentType)
	if err != nil {
		cancel()
		return nil, nil, err
	}
	return resp, func() { resp.Body.Close(); cancel() }, nil
}

func (c *Client) sendRetrying(ctx context.Context, method, bucket, key string, query []pair, body *payload, contentType string) (*http.Response, error) {
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
		resp, err := c.attempt(ctx, method, rawURL, path, query, payloadHash, size, body, contentType)
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

func (c *Client) attempt(ctx context.Context, method, rawURL, path string, query []pair, payloadHash string, size int64, body *payload, contentType string) (*http.Response, error) {
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
	if body != nil {
		req.ContentLength = size
		if size == 0 {
			req.Body = http.NoBody
		}
	}
	req.Header.Set("User-Agent", userAgent)
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	for _, h := range sign(signRequest{
		method:      method,
		host:        c.host,
		path:        path,
		query:       query,
		payloadHash: payloadHash,
		amzDate:     amzDate(time.Now()),
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

// DeleteS3 deletes an object, with a client built from the environment.
func DeleteS3(ctx context.Context, key string) error {
	c, err := defaultC()
	if err != nil {
		return err
	}
	return c.Delete(ctx, key)
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

func sha256New() hash.Hash { return sha256.New() }
