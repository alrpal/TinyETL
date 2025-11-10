# Manual MSSQL Initialization

Since Azure SQL Edge on ARM64 doesn't include sqlcmd or pymssql, you need to manually initialize the database.

## Option 1: Use a SQL Client

Connect to SQL Server with any SQL client (Azure Data Studio, DBeaver, etc.) using:
- Host: `localhost`
- Port: `1433`
- User: `SA`
- Password: `TestPass123!`

Then run:

```sql
-- Create testdb database if not exists
IF NOT EXISTS (SELECT name FROM sys.databases WHERE name = 'testdb')
BEGIN
    CREATE DATABASE testdb;
END
GO

USE testdb;
GO

-- Create testuser login if not exists
IF NOT EXISTS (SELECT name FROM sys.server_principals WHERE name = 'testuser')
BEGIN
    CREATE LOGIN testuser WITH PASSWORD = 'testpass';
END
GO

-- Create testuser in testdb if not exists
IF NOT EXISTS (SELECT name FROM sys.database_principals WHERE name = 'testuser')
BEGIN
    CREATE USER testuser FOR LOGIN testuser;
END
GO

-- Grant permissions
ALTER ROLE db_owner ADD MEMBER testuser;
GO
```

## Option 2: Use tinyetl with SA user

You can use tinyetl directly with the SA account. Note that you need to escape the password in shell:

```bash
# Use single quotes to avoid shell interpretation
tinyetl people.csv 'mssql://SA:TestPass123!@localhost:1433/master#people'

# Or escape the ! character
tinyetl people.csv "mssql://SA:TestPass123\!@localhost:1433/master#people"
```

The SA user can create tables in the `master` database or you can create `testdb` first using Option 1.

## Option 3: Install sqlcmd in the container

```bash
docker exec -it tinyetl-mssql bash

# Inside container:
apt-get update
apt-get install -y curl
curl https://packages.microsoft.com/keys/microsoft.asc | apt-key add -
curl https://packages.microsoft.com/config/ubuntu/20.04/prod.list > /etc/apt/sources.list.d/mssql-release.list
apt-get update
ACCEPT_EULA=Y apt-get install -y mssql-tools18

# Then run initialization
/opt/mssql-tools18/bin/sqlcmd -S localhost -U SA -P TestPass123! -i /docker-entrypoint-initdb.d/01-init.sql -C
```
