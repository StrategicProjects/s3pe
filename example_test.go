package s3pe_test

import (
	"context"
	"errors"
	"fmt"
	"log"

	"github.com/StrategicProjects/s3pe"
)

// Credentials and endpoint come from the environment; keys are s3:// URLs.
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
		AccessKey: "GK…",
		SecretKey: "…",
		Bucket:    "my-bucket",
	})
	if err != nil {
		log.Fatal(err)
	}
	ctx := context.Background()
	if _, err := s3.Upload(ctx, "painel.parquet", "etl/painel.parquet", ""); err != nil {
		log.Fatal(err)
	}
	if _, err := s3.Download(ctx, "etl/painel.parquet", "copy.parquet"); err != nil {
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
