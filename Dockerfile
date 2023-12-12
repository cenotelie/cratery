FROM buildpack-deps:22.04-curl
LABEL maintainer="Laurent Wouters <lwouters@cenotelie.fr>" vendor="Cénotélie Opérations SAS"  description="Cratery -- a private cargo registry"
RUN apt-get update && apt-get install -y --no-install-recommends \
		git \
		ssh

# add ssh host key for github.com
RUN mkdir /root/.ssh && ssh-keyscan -t rsa github.com >> /root/.ssh/known_hosts

COPY cratery /
EXPOSE 80 8000
ENTRYPOINT ["/cratery"]