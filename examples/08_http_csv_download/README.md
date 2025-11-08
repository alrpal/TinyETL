# HTTP CSV Download Example

This example demonstrates how to download a CSV file from an HTTP(S) URL and convert it to JSON format using the `--source-type` parameter.

## Key Features Demonstrated

- **HTTP/HTTPS protocol support**: Download files from web URLs
- **Source type specification**: Use `--source-type=csv` when URL doesn't indicate file format
- **Progress tracking**: Visual progress bar during download
- **Format conversion**: CSV to JSON transformation

## What This Example Does

1. Downloads a CSV file from Google Drive using an HTTP URL
2. Uses `--source-type=csv` to specify the file format (since the URL has query parameters, not a .csv extension)
3. Converts the downloaded CSV to JSON format
4. Shows a preview of the first 5 rows

## Running the Example

```bash
./run.sh
```

## URL Format

The example uses a Google Drive direct download URL:
```
https://drive.google.com/uc?id=FILE_ID&export=download
```

This type of URL doesn't have a clear file extension, so we use `--source-type=csv` to tell TinyETL how to parse the downloaded content.

## Expected Output

The command will:
- Show download progress with a progress bar
- Display a preview of the CSV data
- Create a `people.json` file with the converted data
