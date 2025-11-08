#!/bin/bash

# Example: Download CSV file from HTTP URL using source type specification
# This demonstrates how to use the HTTP protocol with --source-type parameter
# when the URL doesn't have a clear file extension

echo "=== HTTP CSV Download Example ==="
echo "Downloading CSV from Google Drive and converting to JSON..."

# The URL points to a CSV file but doesn't have a .csv extension
# so we need to specify --source-type=csv to tell TinyETL how to parse it
../../target/release/tinyetl \
  "https://drive.google.com/uc?id=1phaHg9objxK2MwaZmSUZAKQ8kVqlgng4&export=download" \
  "people.json" \
  --source-type=csv \
  --preview=5

echo ""
echo "Preview of downloaded and converted data:"
if [ -f "people.json" ]; then
    echo "First few lines of people.json:"
    head -10 people.json
else
    echo "Output file not created - check for errors above"
fi

echo ""
echo "=== HTTP CSV Download Example Complete ==="
