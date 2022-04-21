# syntax = docker/dockerfile:1.2

FROM rust:1.58-bullseye as builder
WORKDIR app
RUN curl -fsSL https://deb.nodesource.com/setup_16.x | bash -
RUN apt-get install -y nodejs
COPY . .

# Install script dependencies
RUN --mount=type=cache,target=./script/node_modules \
    cd ./script && npm install --quiet

# Build CSS
RUN --mount=type=cache,target=./script/node_modules \
    script/build-css --release

# Compile collab server
RUN --mount=type=cache,target=./script/node_modules \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=./target \
    cargo build --release --package collab --bin collab

# Copy collab server binary out of cached directory
RUN --mount=type=cache,target=./target \
    cp /app/target/release/collab /app/collab

# Copy collab server binary to the runtime image
FROM debian:bullseye-slim as runtime
RUN apt-get update; \
    apt-get install -y --no-install-recommends libcurl4-openssl-dev ca-certificates
WORKDIR app
COPY --from=builder /app/collab /app
ENTRYPOINT ["/app/collab"]
