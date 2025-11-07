# TinyETL Test Environment

This Docker Compose setup provides PostgreSQL, MySQL, and MongoDB instances for testing TinyETL functionality.

## Services

| Service   | Port  | Database | Username | Password |
|-----------|-------|----------|----------|----------|
| PostgreSQL| 5432  | testdb   | testuser | testpass |
| MySQL     | 3306  | testdb   | testuser | testpass |
| MongoDB   | 27017 | testdb   | testuser | testpass |

## Quick Start

```bash
# Start all services
docker-compose up -d

# Check service status
docker-compose ps

# View logs
docker-compose logs [service-name]

# Stop all services
docker-compose down

# Stop and remove volumes (clean slate)
docker-compose down -v
```

## Connection Strings for TinyETL

```bash
# PostgreSQL
tinyetl source.csv "postgres://testuser:testpass@localhost:5432/testdb#employees"

# MySQL
tinyetl source.csv "mysql://testuser:testpass@localhost:3306/testdb#employees"

# SQLite (for comparison)
tinyetl source.csv "sqlite://./test.db#employees"
```

## Manual Database Access

### PostgreSQL
```bash
# Connect using psql
docker exec -it tinyetl-postgres psql -U testuser -d testdb

# Or from host (requires psql client)
psql -h localhost -U testuser -d testdb
```

### MySQL
```bash
# Connect using mysql client
docker exec -it tinyetl-mysql mysql -u testuser -ptestpass testdb

# Or from host (requires mysql client)
mysql -h localhost -u testuser -ptestpass testdb
```

### MongoDB
```bash
# Connect using mongosh
docker exec -it tinyetl-mongodb mongosh --username testuser --password testpass --authenticationDatabase admin testdb

# Or from host (requires mongosh client)
mongosh "mongodb://testuser:testpass@localhost:27017/testdb?authSource=admin"
```

## Sample Data

Each database is initialized with sample `employees` and `products` tables/collections:

- **employees**: id, first_name, last_name, email, department, salary, hire_date
- **products**: id, name, category, price, stock_quantity, created_at

## Testing TinyETL Examples

```bash
# Test PostgreSQL connection (from tinyetl root)
cargo run -- examples/05_csv_to_sqlite/employees.csv "postgres://testuser:testpass@localhost:5432/testdb#test_employees" --dry-run

# Test MySQL connection
cargo run -- examples/07_csv_to_mysql/customers.csv "mysql://testuser:testpass@localhost:3306/testdb#test_customers" --dry-run

# Actual data transfer (remove --dry-run)
cargo run -- examples/05_csv_to_sqlite/employees.csv "postgres://testuser:testpass@localhost:5432/testdb#test_employees"
```

## Troubleshooting

### Check if services are running
```bash
docker-compose ps
```

### Check service health
```bash
docker-compose exec postgres pg_isready -U testuser -d testdb
docker-compose exec mysql mysqladmin ping -h localhost -u testuser -ptestpass
docker-compose exec mongodb mongosh --eval "db.adminCommand('ping')"
```

### Reset everything
```bash
docker-compose down -v
docker-compose up -d
```
