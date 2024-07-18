ARG BUILD_FLAGS=
ARG BUILD_TARGET=debug


## Base image with Rust toolchain and dependencies
FROM buildpack-deps:24.04-curl AS base
LABEL maintainer="Laurent Wouters <lwouters@cenotelie.fr>" vendor="Cénotélie Opérations SAS"  description="Cratery -- a private cargo registry"
# add packages
RUN apt-get update && apt-get install -y --no-install-recommends \
		build-essential \
		pkg-config \
		libssl-dev \
		libpq-dev \
		libsqlite3-0 \
		libsqlite3-dev \
		musl-tools \
		git \
		ssh

# add custom user
RUN usermod -l cratery -d /home/cratery ubuntu && mv /home/ubuntu /home/cratery
ENV HOME=/home/cratery
USER cratery
# Add support for Rust
ENV PATH="/home/cratery/.cargo/bin:${PATH}"
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
	&& rustup toolchain install nightly \
	&& rustup default nightly \
	&& rm -rf /home/cratery/.cargo/registry \
	&& mkdir /home/cratery/.cargo/registry
# add ssh host key for github.com
RUN mkdir /home/cratery/.ssh && ssh-keyscan -t rsa github.com >> /home/cratery/.ssh/known_hosts
RUN chmod -R go-rwx /home/cratery/.ssh



## Builder to build the application
FROM base AS builder
ARG BUILD_FLAGS
COPY --chown=cratery . /home/cratery/src
RUN cd /home/cratery/src && cargo +stable build ${BUILD_FLAGS}



## Final target from the base with the application's binary
FROM base
ARG BUILD_TARGET
COPY --from=builder /home/cratery/src/target/${BUILD_TARGET}/cratery /
ENTRYPOINT ["/cratery"]
