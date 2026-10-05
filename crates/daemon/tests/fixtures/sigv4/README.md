# SigV4 vectors

`aws-c-auth/` holds the header-signing cases of the `v4` directory of the aws-c-auth signing
test suite, copied unchanged from
https://github.com/awslabs/aws-c-auth/tree/3281f8692e6fd10562c4585a4dded5c16b322698/tests/aws-signing-test-suite/v4
(Apache License 2.0). Each case keeps `context.json`, `request.txt`, and the four `header-*`
expectations of the header-signing form the MemoryReviewer uses.

`bedrock-colon.json` is a local case for the Bedrock path, produced by botocore's `SigV4Auth`
(its `provenance` field names the version): a model id whose `:` is `%3A` on the wire is
`%253A` in the canonical URI, as botocore builds the canonical request for a non-S3 service.
