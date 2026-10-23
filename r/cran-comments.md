## Submission

This is a new submission: a small client for the S3 object store of the
State of Pernambuco, Brazil (`s3.pe.gov.br`, a Garage server), usable with
any S3-style store.

## R CMD check results

0 errors | 0 warnings | 1 note locally (macOS, R 4.6.0, `--as-cran`): the
note is "New submission". 0 errors | 0 warnings | 0 notes on GitHub Actions
(Windows, macOS, Ubuntu release/devel/oldrel-1).

## Reverse dependencies

There are no reverse dependencies.

## Notes for reviewers

* The store requires credentials for every request, so no example or
  vignette chunk can run on CRAN: examples are guarded with
  `@examplesIf interactive()` and the vignette is not evaluated.
* The test suite runs fully offline against a fake server through
  `httr2::local_mocked_responses()`; the one live test is skipped unless
  the environment variable `S3PE_TEST_BUCKET` is set.
* Network failures raise classed errors (not warnings) by design: the
  package moves data a pipeline depends on, and a silent empty result
  would be worse than a stop. Nothing in the package reaches the
  internet during `R CMD check`.
