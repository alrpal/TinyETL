# Debian 12 with ODBC drivers for SQL Server, PostgreSQL, MySQL/MariaDB and SQLite.
# Used by run.sh to test TinyETL's odbc:// connector with a Linux build of TinyETL, which bundles
# its own unixODBC driver manager (the drivers' own dependencies come from these packages).
FROM debian:12-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates gnupg \
 && curl -fsSL https://packages.microsoft.com/keys/microsoft.asc \
      | gpg --dearmor -o /usr/share/keyrings/microsoft-prod.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/microsoft-prod.gpg] https://packages.microsoft.com/debian/12/prod bookworm main" \
      > /etc/apt/sources.list.d/mssql-release.list \
 && apt-get update \
 && ACCEPT_EULA=Y apt-get install -y --no-install-recommends \
      msodbcsql18 odbc-postgresql odbc-mariadb libsqliteodbc python3 \
 && rm -rf /var/lib/apt/lists/*
