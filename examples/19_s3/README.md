# Example 19: S3 Read/Write

Round-trips data through a real S3 bucket:

1. `employees.csv` → `s3://$TINYETL_S3_BUCKET/tinyetl-example/employees.parquet`
2. That object → `output_employees.json`
3. A CSV round trip from the plain CLI, written twice to confirm overwriting

## Setup

Create a `.env` file at the repository root (it is gitignored):

```bash
TINYETL_S3_BUCKET=your-bucket-name
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
AWS_REGION=us-east-1        # the bucket's region
# AWS_SESSION_TOKEN=...     # only for temporary credentials
```

The credentials need `s3:ListBucket` on the bucket and `s3:GetObject` / `s3:PutObject` on its objects.

## Running

```bash
cargo build --release
./run.sh
```

The script skips itself (exit 0) when the variables are not set. It leaves two small objects under
`tinyetl-example/` in the bucket.
