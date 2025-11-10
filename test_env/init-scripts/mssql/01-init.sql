-- Create database
IF NOT EXISTS (SELECT * FROM sys.databases WHERE name = 'testdb')
BEGIN
    CREATE DATABASE testdb;
END
GO

-- Switch to the testdb database
USE testdb;
GO

-- Create login and user
IF NOT EXISTS (SELECT * FROM sys.server_principals WHERE name = 'testuser')
BEGIN
    CREATE LOGIN testuser WITH PASSWORD = 'testpass';
END
GO

IF NOT EXISTS (SELECT * FROM sys.database_principals WHERE name = 'testuser')
BEGIN
    CREATE USER testuser FOR LOGIN testuser;
END
GO

-- Grant permissions to testuser
ALTER ROLE db_datareader ADD MEMBER testuser;
ALTER ROLE db_datawriter ADD MEMBER testuser;
ALTER ROLE db_ddladmin ADD MEMBER testuser;
GO

PRINT 'Database and user setup completed successfully';
