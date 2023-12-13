FROM buildpack-deps:22.04-curl
LABEL maintainer="Laurent Wouters <lwouters@cenotelie.fr>" vendor="Cénotélie Opérations SAS"  description="Cratery -- a private cargo registry"
RUN apt-get update && apt-get install -y --no-install-recommends \
		build-essential \
		git \
		ssh

# Add support for Rust
ENV PATH="/root/.cargo/bin:${PATH}"
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
	&& rm -rf /root/.cargo/registry \
	&& mkdir /root/.cargo/registry

# add ssh host key for github.com
RUN mkdir /root/.ssh && ssh-keyscan -t rsa github.com >> /root/.ssh/known_hosts
RUN chmod -R go-rwx /root/.ssh

COPY cratery /
EXPOSE 80 8000
ENTRYPOINT ["/cratery"]