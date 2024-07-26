#!/bin/bash

PROXY_HOME="/haentgl"
WORKS="${WORKS:-4}"
PORT="${PORT:-3310}"
TARGET="${TARGET:-x86_64}"
BACKEND_ADDR="${BACKEND_ADDR:-127.0.0.1:3306}"
BINARY_PATH=${PROXY_HOME}"/bin/my-proxy-"${TARGET}

bin_exists() {
  local arch=$(uname -m)
  local binary_path

  case "$arch" in
    "x86_64")
      if [[ -f "${PROXY_HOME}/bin/my-proxy-x86_64" ]]; then
        binary_path="${PROXY_HOME}/bin/my-proxy-x86_64"
      else
        binary_path="${PROXY_HOME}/bin/my-proxy"
      fi
      ;;
    "aarch64")
      if [[ -f "${PROXY_HOME}/bin/my-proxy-aarch64" ]]; then
        binary_path="${PROXY_HOME}/bin/my-proxy-aarch64"
      else
        binary_path="${PROXY_HOME}/bin/my-proxy"
      fi
      ;;
    *)
      binary_path="${PROXY_HOME}/bin/my-proxy"
      ;;
  esac

  echo "Binary path set to ${binary_path}"
  export BINARY_PATH="${binary_path}"
}


start_proxy_with_static_backend() {
  "${BINARY_PATH}" --works "${WORKS}" --port "${PORT}" backend --backend-addr "${BACKEND_ADDR}"
}

start_proxy_with_cp() {
  "${BINARY_PATH}" --works "${WORKS}" --port "${PORT}" --enable-cp backend --backend-addr "${BACKEND_ADDR}"
}

main() {
  echo "Starting proxy WORKS=${WORKS} ENABLE_CP=${ENABLE_CP} PORT=${PORT} BACKEND_ADDR=${BACKEND_ADDR}, TARGET=${TARGET}"
  bin_exists
  if [[ "${ENABLE_CP}" == "true" ]]; then
    echo "Starting proxy with cp"
    start_proxy_with_cp
  else
    echo "Starting proxy with static backend"
    start_proxy_with_static_backend
  fi
}

main
