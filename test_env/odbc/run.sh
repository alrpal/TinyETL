#!/bin/bash
# Test TinyETL's odbc:// connector on Linux against SQL Server, PostgreSQL, MySQL and SQLite.
#
#   cd test_env && docker-compose up -d     # the database containers
#   test_env/odbc/run.sh                    # build a Linux binary, then run the tests
#   test_env/odbc/run.sh --no-build         # reuse the last Linux build
#
# The Linux binary is built from the current working tree inside a rust:bookworm container
# (no system OpenSSL or unixODBC, like the release build) and kept in Docker volumes, so later
# builds are incremental. The drivers come from tinyetl-odbc-drivers (Dockerfile here).
# Installing msodbcsql18 means accepting Microsoft's EULA.

set -euo pipefail
cd "$(dirname "$0")"
REPO=$(cd ../.. && pwd)
NETWORK=tinyetl-network
RUST_IMAGE=rust:1.100-bookworm

for c in tinyetl-mssql tinyetl-postgres tinyetl-mysql; do
    if ! docker ps --format '{{.Names}}' | grep -qx "$c"; then
        echo "⚠️  $c is not running. Start the databases with: cd test_env && docker-compose up -d"
        exit 1
    fi
done

if [ "${1:-}" != "--no-build" ]; then
    echo "Building the Linux binary (first build takes several minutes; needs ~6 GB of Docker memory)..."
    docker volume create tinyetl-linux-build >/dev/null
    docker volume create tinyetl-linux-target >/dev/null
    # The working tree as it is (tracked and new files, uncommitted changes), minus anything
    # gitignored such as target/
    (cd "$REPO" && git ls-files -z --cached --others --exclude-standard |
        while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done |
        tar --null -T - -cf -) | docker run --rm -i \
        -v tinyetl-linux-build:/out -v tinyetl-linux-target:/src/target "$RUST_IMAGE" bash -c '
        set -eo pipefail
        mkdir -p /src && tar -xf - -C /src && cd /src
        # No system OpenSSL, unixODBC or libltdl, like a clean machine
        apt-get remove -y -qq libssl-dev unixodbc-dev libltdl-dev >/dev/null 2>&1 || true
        cargo build --release --locked 2>&1 | tail -1
        cp target/release/tinyetl /out/'
fi

echo "Building the driver image..."
docker build -q -t tinyetl-odbc-drivers . >/dev/null

echo "Dropping old test tables..."
docker exec tinyetl-mssql /opt/mssql-tools18/bin/sqlcmd -S localhost -U SA -P 'TestPass123!' -C -d testdb \
    -Q "IF OBJECT_ID('dbo.tinyetl_odbc_test', 'U') IS NOT NULL DROP TABLE dbo.tinyetl_odbc_test" >/dev/null
docker exec tinyetl-postgres psql -U testuser -d testdb -qc "DROP TABLE IF EXISTS tinyetl_odbc_test" 2>/dev/null
docker exec tinyetl-mysql mysql -utestuser -ptestpass testdb -e "DROP TABLE IF EXISTS tinyetl_odbc_test" 2>/dev/null

docker run --rm --network "$NETWORK" \
    -v tinyetl-linux-build:/dist:ro -v "$PWD":/odbc:ro \
    tinyetl-odbc-drivers bash /odbc/tests.sh
