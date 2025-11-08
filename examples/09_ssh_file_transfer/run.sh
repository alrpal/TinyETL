#!/bin/bash

# Example: Download file via SSH/SCP protocol
# This demonstrates how to use the SSH protocol for secure file transfers
# NOTE: This example requires SSH key authentication or password-less access

echo "=== SSH File Transfer Example ==="

# Check if SSH_HOST and SSH_USER are set
if [ -z "$SSH_HOST" ] || [ -z "$SSH_USER" ]; then
    echo "⚠️  This example requires SSH credentials."
    echo "Set SSH_HOST and SSH_USER environment variables:"
    echo "  export SSH_HOST=example.com"
    echo "  export SSH_USER=username"
    echo ""
    echo "Also ensure you have SSH key authentication set up."
    echo "Example SSH URL format: ssh://user@host/path/to/file.csv"
    echo ""
    echo "For testing, you could use:"
    echo "  ssh://user@localhost/path/to/test.csv (if you have a local SSH server)"
    echo ""
    echo "Skipping SSH example due to missing credentials."
    exit 0
fi

# Construct SSH URL
SSH_URL="ssh://$SSH_USER@$SSH_HOST/tmp/test_data.csv"

echo "Attempting to download file via SSH:"
echo "Source: $SSH_URL"
echo "Target: ssh_download.json"
echo ""

# Create a test CSV file on the remote host (optional)
echo "Creating test file on remote host (if possible)..."
ssh -o StrictHostKeyChecking=no "$SSH_USER@$SSH_HOST" 'echo "name,age,city
Alice,25,New York
Bob,30,Los Angeles
Charlie,35,Chicago" > /tmp/test_data.csv' 2>/dev/null || echo "Could not create test file (SSH access required)"

echo ""
echo "Downloading via TinyETL SSH protocol..."

# Download and convert using TinyETL
../../target/release/tinyetl \
  "$SSH_URL" \
  "ssh_download.json" \
  --preview=5

echo ""
echo "Results:"
if [ -f "ssh_download.json" ]; then
    echo "✅ SSH download successful!"
    echo "Downloaded data:"
    cat ssh_download.json
    
    # Clean up
    rm -f ssh_download.json
else
    echo "❌ SSH download failed - check SSH access and file path"
fi

echo ""
echo "=== SSH File Transfer Example Complete ==="
