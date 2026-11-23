# Example 18: HTTP Source with Custom Headers and Authentication

This example demonstrates how to download data from HTTP/HTTPS endpoints with custom headers and authentication in TinyETL using the local test HTTP server.

## What This Example Shows

1. **Public Endpoints**: Downloading from public HTTP endpoints
2. **Custom HTTP Headers**: Adding custom headers to HTTP requests
3. **Basic Authentication**: Using username/password authentication
4. **Bearer Token Authentication**: Using token-based authentication

## Prerequisites

This example requires the HTTP test server to be running:

```bash
# From the test_env directory
cd ../../test_env
docker-compose up -d http-server

# Verify it's running
curl http://localhost:8680/health
```

## Protocol Options

TinyETL supports the following options for HTTP/HTTPS sources:

| Option | Description | Example |
|--------|-------------|---------|
| `header.<name>` | Custom HTTP header | `header.User-Agent: "TinyETL/0.9.0"` |
| `auth.basic.username` | Basic auth username | `auth.basic.username: "testuser"` |
| `auth.basic.password` | Basic auth password | `auth.basic.password: "testpass"` |
| `auth.bearer` | Bearer token | `auth.bearer: "test-bearer-token-12345"` |

## Available Configurations

### 1. Public Endpoint (No Authentication)
**File**: `public_config.yaml`

Downloads from a public endpoint that requires no authentication.

```yaml
source:
  uri: "http://localhost:8680/public/data.csv"
  options:
    header.User-Agent: "TinyETL/0.9.0"
target: output_public.json
```

### 2. Basic Authentication
**File**: `config.yaml`

Downloads using HTTP Basic Authentication.

```yaml
source:
  uri: "http://localhost:8680/basic-auth/users.csv"
  options:
    auth.basic.username: "testuser"
    auth.basic.password: "testpass"
target: output_basic_auth.json
```

### 3. Bearer Token Authentication
**File**: `bearer_config.yaml`

Downloads using Bearer Token authentication.

```yaml
source:
  uri: "http://localhost:8680/bearer-auth/products.csv"
  options:
    auth.bearer: "test-bearer-token-12345"
target: output_bearer_auth.json
```

### 4. Custom Headers Validation
**File**: `custom_headers_config.yaml`

Downloads from an endpoint that validates specific headers.

```yaml
source:
  uri: "http://localhost:8680/custom-headers/data.csv"
  options:
    header.X-API-Version: "v2"
    header.User-Agent: "TinyETL/0.9.0"
target: output_custom_headers.json
```

## Running the Example

### Run All Tests

```bash
./run.sh
```

This will:
1. Check if the HTTP server is running
2. Test public endpoint (no auth)
3. Test Basic Authentication
4. Test Bearer Token Authentication
5. Test Custom Headers validation

### Run Individual Configurations

```bash
# Public endpoint
tinyetl run public_config.yaml

# Basic authentication
tinyetl run config.yaml

# Bearer token
tinyetl run bearer_config.yaml

# Custom headers
tinyetl run custom_headers_config.yaml
```

## Test Server Endpoints

The HTTP test server provides these endpoints:

| Endpoint | Authentication | Data |
|----------|----------------|------|
| `/public/data.csv` | None | User data CSV |
| `/basic-auth/users.csv` | Basic Auth | User data CSV |
| `/bearer-auth/products.csv` | Bearer Token | Products CSV |
| `/custom-headers/data.csv` | Header validation | User data CSV |

## For Real-World Usage

When using with external APIs, follow these patterns:

### With Environment Variables (Recommended)

```yaml
source:
  uri: "https://api.example.com/data.csv"
  options:
    auth.basic.username: "${API_USERNAME}"
    auth.basic.password: "${API_PASSWORD}"
```

```bash
export API_USERNAME=your_username
export API_PASSWORD=your_password
tinyetl run config.yaml
```

### With Bearer Token

```yaml
source:
  uri: "https://api.example.com/data.json"
  options:
    auth.bearer: "${API_TOKEN}"
    header.Accept: "application/json"
```

```bash
export API_TOKEN=your_token_here
tinyetl run config.yaml
```

## Security Best Practices

1. ✅ **Use environment variables** for credentials
2. ✅ **Never commit credentials** to version control
3. ✅ **Use HTTPS** for production endpoints
4. ✅ **Rotate tokens regularly** for bearer authentication
5. ✅ **Add `.env` files to `.gitignore`**

## Common Use Cases

- **REST API Data Export**: Download data exports from REST APIs
- **Authenticated Webhooks**: Process data from authenticated webhook endpoints  
- **Cloud Storage**: Download files from cloud storage with signed URLs
- **Data Feeds**: Subscribe to authenticated data feeds
- **Internal APIs**: Access internal company APIs with authentication

## Troubleshooting

### Server Not Running
```bash
cd ../../test_env
docker-compose up -d http-server
docker-compose logs http-server
```

### Authentication Failures
```bash
# Test endpoints manually
curl http://localhost:8680/health
curl -u testuser:testpass http://localhost:8680/basic-auth/users.csv
curl -H "Authorization: Bearer test-bearer-token-12345" \
     http://localhost:8680/bearer-auth/products.csv
```

### Clean Up
```bash
rm output_*.json
```

## Notes

- HTTP targets (uploading) are not currently supported
- File downloads are cached temporarily during processing
- The test server runs on `localhost:8680`
- All test credentials are in plain text (for testing only)
