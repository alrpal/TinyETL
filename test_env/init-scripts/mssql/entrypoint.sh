#!/bin/bash
set -e

# Start SQL Server in background
echo "Starting SQL Server..."
/opt/mssql/bin/sqlservr &
SQL_PID=$!

# Run Python initialization script in background
if [ -f "/docker-entrypoint-initdb.d/init_db.py" ]; then
    python3 /docker-entrypoint-initdb.d/init_db.py &
fi

# Keep SQL Server running
wait $SQL_PID
