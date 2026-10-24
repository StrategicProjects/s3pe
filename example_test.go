package s3pe_test

import (
	"context"
	"errors"
	"fmt"
	"log"
	"time"

	"github.com/StrategicProjects/s3pe"
)

// Endpoint and credentials come from the environment; keys are s3:// URLs.
func Example() {
	ctx := context.Background()
	manifest, err := s3pe.ReadS3(ctx, "s3://my-bucket/data/manifest.json")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(len(manifest))

	if _, err := s3pe.WriteS3(ctx, "s3://my-bucket/reports/table1.csv", []byte("a,b\n1,2\n"), ""); err != nil {
		log.Fatal(err)
	}
	objects, err := s3pe.ListS3(ctx, "s3://my-bucket/data/")
	if err != nil {
		log.Fatal(err)
	}
	for _, o := range objects {
		fmt.Println(o.Size, o.Key)
	}
}

// An explicit client, with a default bucket, keeps the keys short.
func ExampleNew() {
	s3, err := s3pe.New(s3pe.Config{
		Endpoint:  "https://s3.example.org",
		AccessKey: "AKIA…",
		SecretKey: "…",
		Bucket:    "my-bucket",
	})
	if err != nil {
		log.Fatal(err)
	}
	ctx := context.Background()
	if _, err := s3.Upload(ctx, "sales.parquet", "etl/sales.parquet", ""); err != nil {
		log.Fatal(err)
	}
	if _, err := s3.Download(ctx, "etl/sales.parquet", "copy.parquet"); err != nil {
		log.Fatal(err)
	}
}

// A missing object is a typed error.
func ExampleClient_Read() {
	s3, err := s3pe.FromEnv()
	if err != nil {
		log.Fatal(err)
	}
	_, err = s3.Read(context.Background(), "s3://my-bucket/does-not-exist.json")
	fmt.Println(errors.Is(err, s3pe.ErrNotFound))
}

// Metadata and extra headers travel with the object; large bodies go up in parts.
func ExampleClient_WriteWith() {
	s3, err := s3pe.FromEnv()
	if err != nil {
		log.Fatal(err)
	}
	_, err = s3.WriteWith(context.Background(), "s3://my-bucket/reports/report.pdf", []byte("%PDF"), s3pe.PutOptions{
		Metadata: map[string]string{"author": "ana"},
		Headers:  map[string]string{"Cache-Control": "max-age=3600"},
	})
	if err != nil {
		log.Fatal(err)
	}
}

// A server-side copy keeps the source's metadata.
func ExampleClient_Copy() {
	s3, err := s3pe.FromEnv()
	if err != nil {
		log.Fatal(err)
	}
	t, err := s3.Copy(context.Background(), "s3://my-bucket/etl/sales.parquet", "s3://my-bucket/archive/sales-2026.parquet")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(t.URL())
}

// A pre-signed URL lets someone without credentials read the object for a while.
func ExampleClient_Presign() {
	s3, err := s3pe.FromEnv()
	if err != nil {
		log.Fatal(err)
	}
	u, err := s3.Presign("s3://my-bucket/reports/report.pdf", "GET", time.Hour)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(u)
}

// SyncUp uploads what changed, the manifest last.
func ExampleClient_SyncUp() {
	s3, err := s3pe.FromEnv()
	if err != nil {
		log.Fatal(err)
	}
	r, err := s3.SyncUp(context.Background(), "./out", "s3://my-bucket/reports/", s3pe.SyncOptions{Last: "manifest.json"})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(len(r.Transferred), "uploaded,", len(r.Skipped), "unchanged")
}
