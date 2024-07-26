# Docker Image

## Build the runtime image

```shell
docker build -f ./docker/DockerfileRuntimeUbuntu  -t eloqsql-proxy .
docker tag eloqsql-proxy:latest ${YOU_DOCKER_REGISTRY}/eloqsql-proxy:latest
docker push ${YOU_DOCKER_REGISTRY}/eloqsql-proxy:latest
```

## Build the images

This section describes how to publish and build a Docker Image and set up a lightweight integration test environment
locally.

Execute the following command in the project root directory.

```shell
# cd haentgl
docker build -f ./docker/Dockerfile  -t my-proxy .
```

## MyProxy with static MariaDB backend.

The `docker-compose-with-mysqld.yml` not used in production environments, its purpose is for integration testing and
debugging. At startup, it will read the
environment variables in `docker-compose.env`. (BACKEND_IMAGE or MY_PROXY_IMAGE)

Execute the following command in the project root directory.

### Start all components

```shell
# This command needs to be executed only once, and if you start it again, run the 
#  docker-compose --env-file ./docker/docker-compose.env  -f ./docker/docker-compose-with-mysqld.yml start
docker-compose --env-file ./docker/docker-compose.env  -f ./docker/docker-compose-with-mysqld.yml up -d
```

### Checking the status of a process

```shell
docker-compose --env-file ./docker/docker-compose.env  -f ./docker/docker-compose-with-mysqld.yml ps -a
```

### Show logs

```shell
docker-compose --env-file ./docker/docker-compose.env  -f ./docker/docker-compose-with-mysqld.yml logs -f 
```



