# Third-Party Licenses

TinyETL is licensed under the Apache License 2.0 with additional commercial terms (see [LICENSE](LICENSE)).
The Linux and macOS release binaries also contain the following libraries, which are **statically linked**
so that the binary has no runtime dependencies. These libraries are **not** covered by TinyETL's license.
They remain under their own licenses, reproduced in [`licenses/`](licenses/).

The additional commercial licensing terms in `LICENSE` do not apply to these libraries.

The Windows binary does not include them. It uses the ODBC driver manager built into Windows.
The Linux binary additionally contains OpenSSL (see below); the macOS and Windows binaries use the
operating system's TLS libraries instead.

## unixODBC

- **What:** the unixODBC driver manager, which TinyETL's `odbc://` connector uses to load ODBC drivers
- **Version:** 2.3.14
- **License:** GNU Lesser General Public License v2.1 or later ([`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt))
- **Copyright:** the unixODBC authors (see `AUTHORS` in the unixODBC source)
- **Source used for the binaries:** the [`unix-odbc` crate, version 0.1.4](https://crates.io/crates/unix-odbc/0.1.4),
  which vendors the unmodified unixODBC 2.3.14 sources. The exact crate version for each release is pinned in that
  release's `Cargo.lock`, and the crate archive on crates.io is immutable. The upstream source is
  [unixODBC-2.3.14.tar.gz](https://www.unixodbc.org/unixODBC-2.3.14.tar.gz).

## GNU libltdl

- **What:** the dynamic-loading library from GNU Libtool, which unixODBC uses to open ODBC driver libraries
- **License:** GNU Lesser General Public License v2 or later ([`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt))
- **Copyright:** Free Software Foundation, Inc.
- **Source used for the binaries:** the libltdl static library from the build machine (the `libltdl-dev` package on
  Linux, the Homebrew `libtool` formula on macOS). The exact version in each binary is recorded in
  `licenses/BUILD-INFO.txt` inside that release download. Source code for any GNU Libtool version is available from
  <https://ftp.gnu.org/gnu/libtool/>.

## OpenSSL (Linux binary only)

- **What:** the TLS library used for encrypted connections by the PostgreSQL, MySQL, MSSQL and HTTP(S) connectors
- **License:** Apache License 2.0 ([`licenses/OpenSSL-LICENSE.txt`](licenses/OpenSSL-LICENSE.txt))
- **Copyright:** The OpenSSL Project Authors
- **Source used for the binaries:** the [`openssl-src` crate](https://crates.io/crates/openssl-src), which vendors
  the unmodified OpenSSL sources. The exact version is pinned in each release's `Cargo.lock` and recorded in
  `licenses/BUILD-INFO.txt` in the Linux download. Upstream source: <https://www.openssl.org/source/>.

## Your rights under the LGPL

You may modify these libraries and use your modified versions with TinyETL. Because they are statically linked,
the LGPL requires that you be able to relink TinyETL against a modified version. TinyETL's complete source code
is public, and every release is tagged in this repository with its `Cargo.lock`, so you can rebuild any released
binary:

1. Check out the release tag, for example `git checkout v0.10.0`.
2. Point the build at your modified unixODBC by adding a patch to `Cargo.toml`:

   ```toml
   [patch.crates-io]
   unix-odbc = { path = "../my-unix-odbc" }
   ```

   For a modified libltdl, put your `libltdl.a` first on the linker path instead:
   `RUSTFLAGS="-L /path/to/my/libltdl/lib"`.
3. Build with `cargo build --release --locked`.

If you need help obtaining the source of a specific release, contact licensing@tinyetl.com.
