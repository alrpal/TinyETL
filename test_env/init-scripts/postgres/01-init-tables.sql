-- Create sample tables for testing TinyETL
CREATE TABLE IF NOT EXISTS employees (
    id SERIAL PRIMARY KEY,
    first_name VARCHAR(50),
    last_name VARCHAR(50),
    email VARCHAR(100) UNIQUE,
    department VARCHAR(50),
    salary DECIMAL(10,2),
    hire_date DATE
);

CREATE TABLE IF NOT EXISTS products (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100),
    category VARCHAR(50),
    price DECIMAL(10,2),
    stock_quantity INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Insert some sample data
INSERT INTO employees (first_name, last_name, email, department, salary, hire_date) VALUES
    ('John', 'Doe', 'john.doe@example.com', 'Engineering', 75000.00, '2023-01-15'),
    ('Jane', 'Smith', 'jane.smith@example.com', 'Marketing', 65000.00, '2023-02-20'),
    ('Bob', 'Johnson', 'bob.johnson@example.com', 'Sales', 55000.00, '2023-03-10')
ON CONFLICT (email) DO NOTHING;

INSERT INTO products (name, category, price, stock_quantity) VALUES
    ('Laptop Pro', 'Electronics', 1299.99, 25),
    ('Office Chair', 'Furniture', 249.50, 50),
    ('Coffee Mug', 'Kitchen', 12.99, 100)
ON CONFLICT DO NOTHING;
