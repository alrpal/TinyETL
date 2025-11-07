// Create sample collections for testing TinyETL
db = db.getSiblingDB('testdb');

// Create employees collection with sample data
db.employees.insertMany([
    {
        _id: 1,
        first_name: "John",
        last_name: "Doe",
        email: "john.doe@example.com",
        department: "Engineering",
        salary: 75000.00,
        hire_date: new Date("2023-01-15")
    },
    {
        _id: 2,
        first_name: "Jane",
        last_name: "Smith",
        email: "jane.smith@example.com",
        department: "Marketing",
        salary: 65000.00,
        hire_date: new Date("2023-02-20")
    },
    {
        _id: 3,
        first_name: "Bob",
        last_name: "Johnson",
        email: "bob.johnson@example.com",
        department: "Sales",
        salary: 55000.00,
        hire_date: new Date("2023-03-10")
    }
], { ordered: false });

// Create products collection with sample data
db.products.insertMany([
    {
        _id: 1,
        name: "Laptop Pro",
        category: "Electronics",
        price: 1299.99,
        stock_quantity: 25,
        created_at: new Date()
    },
    {
        _id: 2,
        name: "Office Chair",
        category: "Furniture",
        price: 249.50,
        stock_quantity: 50,
        created_at: new Date()
    },
    {
        _id: 3,
        name: "Coffee Mug",
        category: "Kitchen",
        price: 12.99,
        stock_quantity: 100,
        created_at: new Date()
    }
], { ordered: false });

print("Sample data inserted into testdb collections");
