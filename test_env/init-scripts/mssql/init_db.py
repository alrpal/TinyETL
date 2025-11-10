#!/usr/bin/env python3
import time
import os

SA_PASSWORD = os.environ.get('SA_PASSWORD', 'TestPass123!')

print("Waiting for SQL Server to be ready...")
time.sleep(20)

# Try using pymssql
try:
    import pymssql
    
    max_attempts = 30
    conn = None
    
    for attempt in range(1, max_attempts + 1):
        try:
            print(f"Attempting to connect to SQL Server (attempt {attempt}/{max_attempts})...")
            conn = pymssql.connect(
                server='localhost',
                user='SA',
                password=SA_PASSWORD,
                database='master',
                timeout=5
            )
            print("✓ Connected to SQL Server!")
            break
        except Exception as e:
            print(f"Connection failed: {e}")
            if attempt < max_attempts:
                time.sleep(3)
    
    if not conn:
        print("Failed to connect to SQL Server")
        print("Please use SA user: mssql://SA:TestPass123!@localhost:1433/master")
        exit(1)
    
    cursor = conn.cursor()
    
    # Create testdb database
    print("Creating testdb database...")
    cursor.execute("""
        IF NOT EXISTS (SELECT name FROM sys.databases WHERE name = 'testdb')
        BEGIN
            CREATE DATABASE testdb;
        END
    """)
    conn.commit()
    print("✓ Database testdb created")
    
    # Create login
    print("Creating testuser login...")
    try:
        cursor.execute("""
            IF NOT EXISTS (SELECT name FROM sys.server_principals WHERE name = 'testuser')
            BEGIN
                CREATE LOGIN testuser WITH PASSWORD = 'testpass';
            END
        """)
        conn.commit()
        print("✓ Login testuser created")
    except Exception as e:
        print(f"Login creation warning: {e}")
    
    # Switch to testdb
    cursor.close()
    conn.close()
    
    conn = pymssql.connect(
        server='localhost',
        user='SA',
        password=SA_PASSWORD,
        database='testdb'
    )
    cursor = conn.cursor()
    
    # Create user in testdb
    print("Creating testuser in testdb...")
    try:
        cursor.execute("""
            IF NOT EXISTS (SELECT name FROM sys.database_principals WHERE name = 'testuser')
            BEGIN
                CREATE USER testuser FOR LOGIN testuser;
            END
        """)
        conn.commit()
        print("✓ User testuser created in testdb")
    except Exception as e:
        print(f"User creation warning: {e}")
    
    # Grant permissions
    print("Granting permissions...")
    cursor.execute("ALTER ROLE db_owner ADD MEMBER testuser")
    conn.commit()
    print("✓ Permissions granted")
    
    cursor.close()
    conn.close()
    
    print("\n" + "="*50)
    print("✓ Database initialization completed successfully!")
    print("="*50)
    print("You can now connect with:")
    print("  mssql://testuser:testpass@localhost:1433/testdb#tablename")
    print("="*50)
    
except ImportError:
    print("pymssql not available")
    print("Please use SA user: mssql://SA:TestPass123!@localhost:1433/master")
except Exception as e:
    print(f"Initialization failed: {e}")
    print("Please use SA user: mssql://SA:TestPass123!@localhost:1433/master")

