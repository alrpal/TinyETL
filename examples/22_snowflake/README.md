# Example 22: Snowflake

Loads data into Snowflake and reads it back, against a real account:

1. A typed CSV (decimals, booleans, NULLs, Unicode, a 9,000-character value) is loaded with **stage + `COPY INTO`** and read back value by value.
2. A second run appends; `--truncate` replaces the rows.
3. The same file is loaded with **batched INSERTs** (`load_method: insert`, the path TinyETL uses automatically on Azure- and GCP-hosted accounts) and read back.
4. 1,000,000 generated rows are loaded (several staged files) and the peak memory is reported.
5. A table TinyETL didn't create is read: `SNOWFLAKE_SAMPLE_DATA.TPCH_SF1.NATION`.

`run.sh` skips itself unless the `SNOWFLAKE_*` variables below are set, so `run_all_examples.sh` still passes without an account.

## One-time setup

### 1. An account

Sign up for a trial at <https://signup.snowflake.com/> and pick **AWS** as the cloud: TinyETL's fast load path (stage + `COPY INTO`) needs an AWS-hosted account. Any edition works.

### 2. A warehouse, database, role and user

Run this in a Snowsight worksheet as `ACCOUNTADMIN`:

```sql
CREATE WAREHOUSE TINYETL_WH WAREHOUSE_SIZE = XSMALL AUTO_SUSPEND = 60 AUTO_RESUME = TRUE
    INITIALLY_SUSPENDED = TRUE;
CREATE DATABASE TINYETL_TEST;
CREATE SCHEMA TINYETL_TEST.EXAMPLES;

CREATE ROLE TINYETL_ROLE;
GRANT USAGE ON WAREHOUSE TINYETL_WH TO ROLE TINYETL_ROLE;
GRANT USAGE ON DATABASE TINYETL_TEST TO ROLE TINYETL_ROLE;
GRANT ALL ON SCHEMA TINYETL_TEST.EXAMPLES TO ROLE TINYETL_ROLE;

-- For step 5. If SNOWFLAKE_SAMPLE_DATA doesn't exist in your account, create it first:
-- CREATE DATABASE SNOWFLAKE_SAMPLE_DATA FROM SHARE SFC_SAMPLES.SAMPLE_DATA;
GRANT IMPORTED PRIVILEGES ON DATABASE SNOWFLAKE_SAMPLE_DATA TO ROLE TINYETL_ROLE;

CREATE USER TINYETL_LOADER TYPE = SERVICE DEFAULT_ROLE = TINYETL_ROLE
    DEFAULT_WAREHOUSE = TINYETL_WH;
GRANT ROLE TINYETL_ROLE TO USER TINYETL_LOADER;
```

`TYPE = SERVICE` makes it a non-human user that signs in with a key pair, no password.

### 3. A key pair

```bash
mkdir -p ~/.snowflake
openssl genrsa 2048 | openssl pkcs8 -topk8 -inform PEM -nocrypt -out ~/.snowflake/tinyetl_rsa_key.p8
openssl rsa -in ~/.snowflake/tinyetl_rsa_key.p8 -pubout -out ~/.snowflake/tinyetl_rsa_key.pub
chmod 600 ~/.snowflake/tinyetl_rsa_key.p8
```

Attach the public key to the user. Paste the contents of `tinyetl_rsa_key.pub` without the `-----BEGIN/END PUBLIC KEY-----` lines:

```sql
ALTER USER TINYETL_LOADER SET RSA_PUBLIC_KEY = 'MIIBIjANBgkqh...';
```

To protect the private key with a passphrase instead, drop `-nocrypt` and also set `SNOWFLAKE_PRIVATE_KEY_PASSPHRASE`.

### 4. Your account identifier

It has the form `ORGNAME-ACCOUNTNAME`. Find it in Snowsight under your profile → *Account* → *View account details*, or run:

```sql
SELECT CURRENT_ORGANIZATION_NAME() || '-' || CURRENT_ACCOUNT_NAME();
```

### 5. The `.env` file

Put these in a `.env` file at the repository root (it is gitignored, so it won't be committed):

```bash
SNOWFLAKE_ACCOUNT=MYORG-MYACCOUNT
SNOWFLAKE_USER=TINYETL_LOADER
SNOWFLAKE_PRIVATE_KEY_PATH=/Users/you/.snowflake/tinyetl_rsa_key.p8
SNOWFLAKE_WAREHOUSE=TINYETL_WH
SNOWFLAKE_DATABASE=TINYETL_TEST
SNOWFLAKE_SCHEMA=EXAMPLES
SNOWFLAKE_ROLE=TINYETL_ROLE
```

## Running it

```bash
cargo build --release
bash examples/22_snowflake/run.sh
```

A full run takes a few minutes of X-Small warehouse time, a small fraction of a trial's free credits. The warehouse suspends itself after 60 seconds idle.

The example leaves three tables behind, which it truncates on the next run. To remove them:

```sql
DROP TABLE TINYETL_TEST.EXAMPLES.TINYETL_EXAMPLE_TYPED;
DROP TABLE TINYETL_TEST.EXAMPLES.TINYETL_EXAMPLE_INSERT;
DROP TABLE TINYETL_TEST.EXAMPLES.TINYETL_EXAMPLE_BIG;
```
