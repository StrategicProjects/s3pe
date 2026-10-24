package s3pe

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Against a real store, only when S3PE_ENDPOINT is set and S3PE_TEST_BUCKET
// names a bucket the credentials in the environment can write to.
// Everything goes under s3pe-test/go-<pid>/ and is deleted at the end.
func TestLiveRoundTrip(t *testing.T) {
	bucket := os.Getenv("S3PE_TEST_BUCKET")
	if bucket == "" || os.Getenv("S3PE_ENDPOINT") == "" {
		t.Skip("S3PE_TEST_BUCKET / S3PE_ENDPOINT not set")
	}
	s3, err := New(Config{Bucket: bucket, MultipartThreshold: 6 << 20, PartSize: 5 << 20})
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	prefix := fmt.Sprintf("s3pe-test/go-%d/", os.Getpid())
	key := prefix + "ação e espaço.csv"
	t.Cleanup(func() {
		if listed, err := s3.List(ctx, prefix); err == nil && len(listed) > 0 {
			var keys []string
			for _, o := range listed {
				keys = append(keys, o.Key)
			}
			s3.DeleteMany(ctx, keys)
		}
	})

	if ok, err := s3.Exists(ctx, key); err != nil || ok {
		t.Fatalf("exists: %v %v", ok, err)
	}
	tr, err := s3.WriteWith(ctx, key, []byte("a,b\n1,2\n"), PutOptions{Metadata: map[string]string{"origin": "live-test"}})
	if err != nil || tr.Size != 8 {
		t.Fatalf("%+v %v", tr, err)
	}
	got, err := s3.Read(ctx, key)
	if err != nil || string(got) != "a,b\n1,2\n" {
		t.Fatalf("%q %v", got, err)
	}
	o, err := s3.Head(ctx, key)
	if err != nil || o == nil || o.Size != 8 || o.ContentType != "text/csv" || o.Metadata["origin"] != "live-test" {
		t.Fatalf("%+v %v", o, err)
	}

	// multipart: 12 MiB above a 6 MiB threshold, 5 MiB parts
	big := make([]byte, 12<<20)
	for i := range big {
		big[i] = byte(i % 253)
	}
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "big.bin"), big, 0o644)
	tr, err = s3.Upload(ctx, filepath.Join(dir, "big.bin"), prefix+"big.bin", "")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasSuffix(tr.ETag, "-3") {
		t.Fatalf("multipart etag %q", tr.ETag)
	}
	if _, err := s3.Download(ctx, prefix+"big.bin", filepath.Join(dir, "back.bin")); err != nil {
		t.Fatal(err)
	}
	back, _ := os.ReadFile(filepath.Join(dir, "back.bin"))
	if !bytes.Equal(back, big) {
		t.Fatal("bytes differ")
	}

	// copy, one-level listing
	if _, err := s3.Copy(ctx, key, prefix+"sub/copy.csv"); err != nil {
		t.Fatal(err)
	}
	if got, _ := s3.Read(ctx, prefix+"sub/copy.csv"); string(got) != "a,b\n1,2\n" {
		t.Fatal("copy differs")
	}
	l, err := s3.ListDir(ctx, prefix)
	if err != nil || len(l.Objects) != 2 || len(l.Prefixes) != 1 || !strings.HasSuffix(l.Prefixes[0], "sub/") {
		t.Fatalf("%+v %v", l, err)
	}

	// pre-signed GET
	u, err := s3.Presign(key, "GET", 5*time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	resp, err := http.Get(u)
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 || string(body) != "a,b\n1,2\n" {
		t.Fatalf("presigned: %d %q", resp.StatusCode, body)
	}

	// sync up and down
	syncDir := filepath.Join(dir, "sync")
	os.MkdirAll(filepath.Join(syncDir, "sub"), 0o755)
	os.WriteFile(filepath.Join(syncDir, "a.txt"), []byte("a"), 0o644)
	os.WriteFile(filepath.Join(syncDir, "sub", "b.txt"), []byte("b"), 0o644)
	os.WriteFile(filepath.Join(syncDir, "manifest.json"), []byte("{}"), 0o644)
	r, err := s3.SyncUp(ctx, syncDir, prefix+"sync/", SyncOptions{Last: "manifest.json"})
	if err != nil || len(r.Transferred) != 3 || r.Transferred[2] != "manifest.json" {
		t.Fatalf("%+v %v", r, err)
	}
	r, err = s3.SyncUp(ctx, syncDir, prefix+"sync/", SyncOptions{})
	if err != nil || len(r.Transferred) != 0 || len(r.Skipped) != 3 {
		t.Fatalf("second sync up: %+v %v", r, err)
	}
	down := filepath.Join(dir, "down")
	r, err = s3.SyncDown(ctx, prefix+"sync/", down, SyncOptions{})
	if err != nil || len(r.Transferred) != 3 {
		t.Fatalf("sync down: %+v %v", r, err)
	}
	if b, _ := os.ReadFile(filepath.Join(down, "sub", "b.txt")); string(b) != "b" {
		t.Fatal("sync down content")
	}

	// batch delete and cleanup
	listed, err := s3.List(ctx, prefix)
	if err != nil || len(listed) != 6 {
		t.Fatalf("%d objects %v", len(listed), err)
	}
	var keys []string
	for _, o := range listed {
		keys = append(keys, o.Key)
	}
	if err := s3.DeleteMany(ctx, keys); err != nil {
		t.Fatal(err)
	}
	if listed, _ := s3.List(ctx, prefix); len(listed) != 0 {
		t.Fatalf("left behind: %+v", listed)
	}
}
