# SSH File Transfer Example

This example demonstrates how to download files using the SSH/SCP protocol with TinyETL.

## Key Features Demonstrated

- **SSH protocol support**: Secure file transfer using SCP
- **Progress tracking**: Visual spinner during SSH transfer
- **Authentication**: Uses SSH key-based authentication
- **Format conversion**: CSV to JSON transformation via SSH

## Prerequisites

- SSH access to a remote server
- SSH key-based authentication set up (password-less access)
- A CSV file available on the remote server

## Environment Variables

Set these environment variables before running:

```bash
export SSH_HOST=example.com        # Remote server hostname
export SSH_USER=username          # SSH username
```

## What This Example Does

1. Checks for required SSH credentials
2. Creates a test CSV file on the remote server (if possible)
3. Downloads the file using SSH protocol: `ssh://user@host/path/to/file.csv`
4. Converts the downloaded CSV to JSON format
5. Shows a preview of the converted data

## Running the Example

```bash
# Set credentials
export SSH_HOST=your-server.com
export SSH_USER=your-username

# Run the example
./run.sh
```

## SSH URL Format

```
ssh://username@hostname:port/path/to/file.csv
```

- `username`: SSH username
- `hostname`: Remote server hostname or IP
- `port`: SSH port (optional, defaults to 22)
- `/path/to/file.csv`: Absolute path to the file on remote server

## Authentication

This example uses SSH key-based authentication. Ensure you have:
1. SSH keys set up (`ssh-keygen` if needed)
2. Public key added to remote server (`ssh-copy-id user@host`)
3. Ability to SSH without password prompt

## Testing Locally

You can test with a local SSH server:

```bash
# If you have SSH server running locally
export SSH_HOST=localhost
export SSH_USER=$USER
./run.sh
```

## Expected Output

The command will:
- Show SSH connection progress with a spinner
- Display a preview of the CSV data
- Create a JSON file with the converted data
- Clean up temporary files
