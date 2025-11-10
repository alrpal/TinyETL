#!/bin/bash
# Initialize MSSQL database and user

echo "Waiting for SQL Server to be ready..."

# Wait for SQL Server to be ready
for i in {1..30}; do
    if docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "SELECT 1" >/dev/null 2>&1; then
        echo "SQL Server is ready!"
        break
    fi
    echo "Waiting for SQL Server... (attempt $i/30)"
    sleep 2
done

echo "Creating database and user..."

# Create the database and user
docker exec tinyetl-mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U SA -P TestPass123! -Q "
IF NOT EXISTS (SELECT * FROM sys.databases WHERE name = 'testdb')
BEGIN
    CREATE DATABASE testdb;
END

USE testdb;

IF NOT EXISTS (SELECT * FROM sys.server_principals WHERE name = 'testuser')
BEGIN
    CREATE LOGIN testuser WITH PASSWORD = 'testpass';
END

IF NOT EXISTS (SELECT * FROM sys.database_principals WHERE name = 'testuser')
BEGIN
    CREATE USER testuser FOR LOGIN testuser;
END

-- Grant permissions to testuser
ALTER ROLE db_datareader ADD MEMBER testuser;
ALTER ROLE db_datawriter ADD MEMBER testuser;
ALTER ROLE db_ddladmin ADD MEMBER testuser;

PRINT 'Database and user setup completed successfully';
"

if [ $? -eq 0 ]; then
    echo "✅ Database and user created successfully!"
    echo "✅ You can now connect using: testuser/testpass to database testdb"
else
    echo "❌ Failed to create database and user"
    exit 1
fi
