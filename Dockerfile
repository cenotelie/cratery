ARG RUST_VERSION=1.91.1
ARG BUILD_TARGET=debug
ARG BUILD_FLAGS=""

FROM rust:${RUST_VERSION}-slim AS buildrust
WORKDIR /app

RUN <<EOF
apt-get update
apt-get install --no-install-recommends -y pkg-config libsqlite3-0 git
EOF

RUN --mount=type=bind,source=src/,target=./src \
    --mount=type=bind,source=build.rs/,target=./build.rs \
    --mount=type=bind,source=Cargo.toml/,target=./Cargo.toml \
    --mount=type=bind,source=Cargo.lock/,target=./Cargo.lock \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    <<EOF
set -e
cargo build --release --locked
cp ./target/release/cratery /bin/cratery
EOF


FROM rust:${RUST_VERSION}-slim AS final

RUN <<EOF
apt-get update
apt-get install --no-install-recommends -y ssh git libsqlite3-0
EOF

# Create a non-privileged user that the app will run under.
# See https://docs.docker.com/develop/develop-images/dockerfile_best-practices/#user
ARG UID=10000
RUN adduser \
    --disabled-password \
    --gecos "" \
    --shell "/sbin/nologin" \
    --uid "${UID}" \
    cratery

# Copy the executable from the "build" stage.
COPY --from=buildrust /bin/cratery /bin/

# Create directories the shall be used
RUN mkdir /data && chown -R cratery:cratery /data

USER cratery
WORKDIR /home/cratery

RUN <<EOF
set -e
rustup toolchain install nightly
rustup default nightly
mkdir -p /home/cratery/.cargo/registry
# add ssh host key for github.com
set -e
mkdir /home/cratery/.ssh && ssh-keyscan -t rsa github.com >> /home/cratery/.ssh/known_hosts
chmod -R go-rwx /home/cratery/.ssh
EOF

ENTRYPOINT ["/bin/cratery"]
