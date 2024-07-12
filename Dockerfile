FROM buildpack-deps:22.04-curl AS base
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
RUN adduser cratery
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

FROM base AS builder
COPY --chown=cratery . /home/cratery/src
RUN cd /home/cratery/src && cargo +stable build --release

FROM base
COPY --from=builder /home/cratery/src/target/release/cratery /
ENTRYPOINT ["/cratery"]
