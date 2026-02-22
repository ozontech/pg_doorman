# Installing PgDoorman

This guide covers different methods for installing and running PgDoorman on your system.

## System Requirements

- Linux (recommended) or macOS
- PostgreSQL server (version 10 or higher)
- Sufficient memory for connection pooling (depends on expected load)

## Installation Methods

### Pre-built Binaries (Recommended)

The simplest way to install PgDoorman is to download a pre-built binary from the [GitHub releases page](https://github.com/ozontech/pg_doorman/releases).

1. Download the appropriate binary for your platform
2. Make the file executable: `chmod +x pg_doorman`
3. Move it to a directory in your PATH: `sudo mv pg_doorman /usr/local/bin/`
4. Create a configuration file (see [Basic Usage](./basic-usage.md) for details)

### Building from Source

If you prefer to build from source, you'll need to clone the repository first:

```bash
git clone https://github.com/ozontech/pg_doorman.git
cd pg_doorman
```

Then follow the instructions in the [Contributing guide](./contributing.md) to build the project.

## Docker Installation

### Using the Official Docker Image (Recommended)

PgDoorman provides an official Docker image that you can use directly:

```bash
# Pull the official Docker image
docker pull ghcr.io/ozontech/pg_doorman

# Run PgDoorman with your configuration
docker run -p 6432:6432 \
  -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  --rm -t -i ghcr.io/ozontech/pg_doorman
```

### Using the Dockerfile

You can build and run PgDoorman using Docker:

```bash
# Build the Docker image
docker build -t pg_doorman -f Dockerfile .

# Run PgDoorman with your configuration
docker run -p 6432:6432 \
  -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  --rm -t -i pg_doorman
```

### Using Nix with Docker

If you use Nix, you can build a Docker image:

```bash
# Build the Docker image using Nix
nix build .#dockerImage

# Load the image into Docker
docker load -i result

# Run PgDoorman with your configuration
docker run -p 6432:6432 \
  -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  --rm -t -i pg_doorman
```

## Using Docker Compose or Podman Compose

For a more complete setup including PostgreSQL, you can use Docker Compose or Podman Compose.

A minimal compose configuration file is available in the [repository examples directory](https://github.com/ozontech/pg_doorman/tree/master/example).

### Running with Docker Compose

```bash
docker compose up -d
```

### Running with Podman Compose

```bash
podman-compose up -d
```

## Verifying Installation

After installation, you can verify that PgDoorman is running correctly by:

1. Checking the process: `ps aux | grep pg_doorman`
2. Connecting to the admin console: `psql -h localhost -p 6432 -U admin pgdoorman`
3. Running `SHOW VERSION;` in the admin console

## Next Steps

After installation, see the [Basic Usage guide](./basic-usage.md) to configure and start using PgDoorman.