# Use Ubuntu 22.04 as the base image
FROM ubuntu:22.04 as base

# Install required packages for building and cross-compilation
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y  --no-install-recommends mariadb-test \
    ca-certificates \
    build-essential \
    protobuf-compiler libprotobuf-dev \
    curl \
    gcc gcc-aarch64-linux-gnu gcc-x86-64-linux-gnu \
    libc6-dev-amd64-cross  libc6-dev-arm64-cross \
    && apt-get clean && \
    rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

COPY ../mysql-tests/my_proxy  /usr/share/mysql/mysql-test/suite/my_proxy
COPY ../mysql-tests/run-my-proxy-test.bash  /usr/share/mysql/mysql-test/run-my-proxy-test.bash
# Install Rust using rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --no-modify-path --default-toolchain none -y

# Set the environment variables for Rust
ENV PATH /root/.cargo/bin/:$PATH
ENV CARGO_INCREMENTAL=0

COPY rust-toolchain rust-toolchain
COPY . /haentgl
# RUN rustup override set nightly
# Set the working directory inside the container
WORKDIR /haentgl
RUN  mkdir -p /haentgl/bin /haentgl/etc  /haentgl/lib

# Build the MyProxy binary
RUN cargo fetch && \
    cargo build --release && \
     cp /haentgl/target/release/my-proxy /haentgl/bin && \
     cp /haentgl/target/release/build/tikv-jemalloc-sys-*/out/build/bin/jeprof /haentgl/bin && \
    chmod +x /haentgl/bin/jeprof && cargo clean

# COPY --from=base /haentgl/bin/* /bin

ENV PROXY_HOME  /haentgl
ENV BINARY_NAME /haentgl/bin/my-proxy

COPY ./docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]