package s3pe

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

// Against the real server, only when S3PE_TEST_BUCKET names a bucket the
// credentials in the environment can write to. Everything goes under
// s3pe-test/<pid>/ and is deleted at the end.
func TestLiveRoundTrip(t *testing.T) {
	bucket := os.Getenv("S3PE_TEST_BUCKET")
	if bucket == "" {
		t.Skip("S3PE_TEST_BUCKET not set")
	}
	s3, err := New(Config{Bucket: bucket})
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	prefix := fmt.Sprintf("s3pe-test/go-%d/", os.Getpid())
	key := prefix + "ação e espaço.csv"

	if ok, err := s3.Exists(ctx, key); err != nil || ok {
		t.Fatalf("exists: %v %v", ok, err)
	}
	tr, err := s3.Write(ctx, key, []byte("a,b\n1,2\n"), "")
	if err != nil || tr.Size != 8 {
		t.Fatalf("%+v %v", tr, err)
	}
	got, err := s3.Read(ctx, key)
	if err != nil || string(got) != "a,b\n1,2\n" {
		t.Fatalf("%q %v", got, err)
	}
	o, err := s3.Head(ctx, key)
	if err != nil || o == nil || o.Size != 8 || o.ContentType != "text/csv" {
		t.Fatalf("%+v %v", o, err)
	}

	big := make([]byte, 2_500_000)
	for i := range big {
		big[i] = byte(i % 253)
	}
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "big.bin"), big, 0o644)
	if _, err := s3.Upload(ctx, filepath.Join(dir, "big.bin"), prefix+"big.bin", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := s3.Download(ctx, prefix+"big.bin", filepath.Join(dir, "back.bin")); err != nil {
		t.Fatal(err)
	}
	back, _ := os.ReadFile(filepath.Join(dir, "back.bin"))
	if !bytes.Equal(back, big) {
		t.Fatal("bytes differ")
	}

	listed, err := s3.List(ctx, prefix)
	if err != nil || len(listed) != 2 {
		t.Fatalf("%+v %v", listed, err)
	}
	for _, o := range listed {
		if err := s3.Delete(ctx, o.Key); err != nil {
			t.Fatal(err)
		}
	}
	if listed, _ := s3.List(ctx, prefix); len(listed) != 0 {
		t.Fatalf("left behind: %+v", listed)
	}
}
