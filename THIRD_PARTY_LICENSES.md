# Third-Party Licenses

TinyETL is licensed under the Apache License 2.0 with additional commercial terms (see [LICENSE](LICENSE)).
The release binaries also contain the following libraries, which are **statically linked** so that the
binary has no runtime dependencies. These libraries are **not** covered by TinyETL's license. They remain
under their own licenses, reproduced in [`licenses/`](licenses/).

The additional commercial licensing terms in `LICENSE` do not apply to these libraries.

| Library | Linux | macOS | Windows |
|---|---|---|---|
| unixODBC (LGPL) | yes | yes | no: uses the ODBC driver manager built into Windows |
| OpenSSL | yes | no: uses the system's TLS | no: uses the system's TLS |
| AWS-LC | yes | yes | yes |

## unixODBC

- **What:** the unixODBC driver manager, which TinyETL's `odbc://` connector uses to load ODBC drivers
- **Version:** 2.3.14
- **License:** GNU Lesser General Public License v2.1 or later ([`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt))
- **Copyright:** the unixODBC authors (see `AUTHORS` in the unixODBC source)
- **Source used for the binaries:** the [`unix-odbc` crate, version 0.1.4](https://crates.io/crates/unix-odbc/0.1.4),
  which vendors the unmodified unixODBC 2.3.14 sources. The crate compiles them with its own small `ltdl.h`, which
  loads drivers with the system's `dlopen()` instead of GNU libltdl, so libltdl is not part of the binary. The exact
  crate version for each release is pinned in that release's `Cargo.lock`, and the crate archive on crates.io is
  immutable. The upstream source is [unixODBC-2.3.14.tar.gz](https://www.unixodbc.org/unixODBC-2.3.14.tar.gz).

## OpenSSL (Linux binary only)

- **What:** the TLS library used for encrypted connections by the PostgreSQL, MySQL, MSSQL and HTTP(S) connectors
- **License:** Apache License 2.0 ([`licenses/OpenSSL-LICENSE.txt`](licenses/OpenSSL-LICENSE.txt))
- **Copyright:** The OpenSSL Project Authors
- **Source used for the binaries:** the [`openssl-src` crate](https://crates.io/crates/openssl-src), which vendors
  the unmodified OpenSSL sources. The exact version is pinned in each release's `Cargo.lock` and recorded in
  `licenses/BUILD-INFO.txt` in the Linux download. Upstream source: <https://www.openssl.org/source/>.

## AWS-LC

- **What:** the cryptography library behind the TLS connections of the Snowflake and S3 connectors (through `rustls`)
- **License:** permissive: ISC, Apache License 2.0, MIT and BSD-3-Clause for different parts
  ([`licenses/AWS-LC-LICENSE.txt`](licenses/AWS-LC-LICENSE.txt))
- **Copyright:** Amazon.com, Inc. or its affiliates; Google Inc. (BoringSSL); the OpenSSL Project and others, as
  listed in the license file
- **Source used for the binaries:** the [`aws-lc-sys` crate](https://crates.io/crates/aws-lc-sys), which vendors the
  AWS-LC sources. The exact version is pinned in each release's `Cargo.lock`. Upstream source:
  <https://github.com/aws/aws-lc>.

## Your rights under the LGPL

You may modify unixODBC and use your modified version with TinyETL. Because it is statically linked,
the LGPL requires that you be able to relink TinyETL against a modified version. TinyETL's complete source code
is public, and every release is tagged in this repository with its `Cargo.lock`, so you can rebuild any released
binary:

1. Check out the release tag, for example `git checkout v0.11.1`.
2. Point the build at your modified unixODBC by adding a patch to `Cargo.toml`:

   ```toml
   [patch.crates-io]
   unix-odbc = { path = "../my-unix-odbc" }
   ```
3. Build with `cargo build --release --locked`.

If you need help obtaining the source of a specific release, contact licensing@tinyetl.com.
