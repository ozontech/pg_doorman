# Installing PgDoorman


As first class option, you can obtain PgDoorman distribution via [GitHub releases](https://github.com/ozontech/pg_doorman/releases).

Any other option implies, that you have already cloned project with [Git](https://git-scm.com/):

`git clone https://github.com/ozontech/pg_doorman.git`


### Docker

```
docker build -t pg_doorman -f Dockerfile .
docker run -p 6432:6432 -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml --rm -t -i pg_doorman
```

## Nix (Docker)

```
nix build .#dockerImage
docker load -i result
docker run -p 6432:6432 -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml --rm -t -i pg_doorman
```

## Docker/Podman Compose

Discover minimal compose configuration file in the [repository directory](https://github.com/ozontech/pg_doorman/tree/master/example)

### Running Docker Compose

```
docker compose up -d
```

### Running Podman Compose

```
podman-compose up -d
```