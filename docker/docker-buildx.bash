#!/bin/bash

PLATFORMS=${PLATFORMS:-"linux/arm64,linux/amd64"}

help() {
  echo "Usage: $0 <docker-image-name> [rust-toolchain]"
  echo ""
  echo "Arguments:"
  echo "  <docker-image-name>   Name of the Docker image to build and push"
  echo "  [rust-toolchain]      Rust toolchain version to use (default: nightly)"
  echo ""
  echo "Environment Variables:"
  echo "  PLATFORMS             Comma-separated list of target platforms (default: linux/arm64,linux/amd64)"
  exit 1
}

if [ -z "$1" ]; then
  help
else
  IMG=$1
fi

RUST_TOOLCHAIN=${2:-"nightly"}
USING_MIRROR=${3:-"false"}

docker_buildx() {
    echo "Building and pushing docker image for platforms: $PLATFORMS"
    echo "IMG: $IMG"
    echo "RUST_TOOLCHAIN: $RUST_TOOLCHAIN"
    # Create a new buildx builder instance if it doesn't exist
    docker buildx create --name project-v3-builder --use || docker buildx use project-v3-builder

    # Build and push the Docker image
    docker buildx build \
           --build-arg RUST_TOOLCHAIN="$RUST_TOOLCHAIN" \
           --build-arg USING_MIRROR="$USING_MIRROR" \
           --push --platform="$PLATFORMS" \
           --tag "$IMG" \
           -f "./docker/DockerfileRuntimeUbuntu" .
    # Clean up the builder instance
    docker buildx rm project-v3-builder || true
}

docker_buildx
