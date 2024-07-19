<div align="center">
  <h1>📦 Cratery</h1>
    <strong>Lightweight private cargo registry with batteries included, built for organisations</strong>
  </a>
  <br>
  <br>

[![Build Status](https://dev.azure.com/cenotelie/cenotelie/_apis/build/status%2Fcenotelie.cratery?branchName=master)](https://dev.azure.com/cenotelie/cenotelie/_build/latest?definitionId=34&branchName=master)
  [![Cratery Crates.io version](https://img.shields.io/crates/v/cratery?style=flat)](https://crates.io/crates/cratery)
  [![Cratery Rust documentation](https://docs.rs/cratery/badge.svg)](https://docs.rs/cratery)
  [![Cratery dependency status](https://deps.rs/repo/github/cenotelie/cratery/status.svg)](https://deps.rs/repo/github/cenotelie/cratery)
  [![docker](https://img.shields.io/docker/v/cenotelie/cratery)](https://hub.docker.com/r/cenotelie/cratery)

</div>


## Quickstart

To launch an empty registry using a pre-built docker image, you can use the following default `docker-compose.yml`:

```yaml
version: '3'
services:
  cratery:
    image: cenotelie/cratery:latest
    restart: unless-stopped
    ports:
      - "80:80"
    volumes:
      - ./data:/data
    environment:
      RUST_BACKTRACE: 1
      # REGISTRY_LOG_LEVEL: INFO
      # REGISTRY_LOG_DATE_TIME_FORMAT: "[%Y-%m-%d %H:%M:%S]"
      # REGISTRY_WEB_LISTENON_IP: "0.0.0.0"
      # REGISTRY_WEB_LISTENON_PORT: 80
      REGISTRY_WEB_COOKIE_SECRET: this is the secret key for my yummy cookies, this is the secret key for my yummy cookies
      REGISTRY_WEB_PUBLIC_URI: http://localhost
      # REGISTRY_WEB_BODY_LIMIT: 10485760
      REGISTRY_DATA_DIR: /data
      # REGISTRY_INDEX_PROTOCOL_GIT: "true"
      # REGISTRY_INDEX_PROTOCOL_SPARSE: "true"
      # REGISTRY_GIT_REMOTE:
      # REGISTRY_GIT_REMOTE_SSH_KEY_FILENAME:
      # REGISTRY_GIT_REMOTE_PUSH_CHANGES:
      REGISTRY_GIT_USER_NAME: Cratery
      REGISTRY_GIT_USER_EMAIL: cratery@acme.org
      REGISTRY_STORAGE: "fs"
      # REGISTRY_STORAGE_TIMEOUT: 3000
      # REGISTRY_S3_URI:
      # REGISTRY_S3_REGION:
      # REGISTRY_S3_SERVICE:
      # REGISTRY_S3_ACCESS_KEY:
      # REGISTRY_S3_SECRET_KEY:
      # REGISTRY_S3_BUCKET:
      REGISTRY_OAUTH_LOGIN_URI: https://accounts.google.com/o/oauth2/v2/auth
      REGISTRY_OAUTH_TOKEN_URI: https://oauth2.googleapis.com/token
      REGISTRY_OAUTH_CALLBACK_URI: http://localhost/webapp/oauthcallback.html
      REGISTRY_OAUTH_USERINFO_URI: https://openidconnect.googleapis.com/v1/userinfo
      REGISTRY_OAUTH_USERINFO_PATH_EMAIL: email
      REGISTRY_OAUTH_USERINFO_PATH_FULLNAME: name
      REGISTRY_OAUTH_CLIENT_ID: 710376957465-81ohjssn6skq68ktblkicqr1f2jd33ai.apps.googleusercontent.com
      REGISTRY_OAUTH_CLIENT_SECRET: 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
      REGISTRY_OAUTH_CLIENT_SCOPE: openid profile email
      # REGISTRY_SELF_LOCAL_NAME: localhost
```


## Features

- Authentication with OAuth
- Fine-grained administration on hosted crates
- Automatic documentation generation (docs.rs like)


## Configuration

Configuration is passed through environment variables.
See `docker-compose.yml` for all values.

### General

* `REGISTRY_WEB_PUBLIC_URI`: The URI at which the registry will be available.
* `REGISTRY_WEB_COOKIE_SECRET`: The secret key for the private cookie set by `cratery` to track connected users.

### Authentication

Authentication on `cratery` is achived with OAuth and configured with the `REGISTRY_OAUTH_*` environment variables.
The [docker-compose.yml](docker-compose.yml) file contains the basic configuration to use Google as the authentication provider.
This is allowed only for `cratery` instances exposed on `localhost` for evaluation and testing purposes.
This configuration must be changed to use your own OAuth identity provider.

* `REGISTRY_OAUTH_LOGIN_URI`: URI to redirect to the when attempting to log in.
* `REGISTRY_OAUTH_CALLBACK_URI`: URI on `cratery` the user will be redirected to on successful login on the identity provider.
* `REGISTRY_OAUTH_TOKEN_URI`: URI `cratery` will connect to for obtaining an authorization token from the identity provider.
* `REGISTRY_OAUTH_USERINFO_URI`: URI `cratery` will connect to for obtaining the user information from the identity provider when a user logged in.
* `REGISTRY_OAUTH_USERINFO_PATH_EMAIL`: The path to the email field in the JSON blob returned by the identity provider as the user information.
* `REGISTRY_OAUTH_USERINFO_PATH_FULLNAME`: The path to the full name field in the JSON blob returned by the identity provider as the user information.
* `REGISTRY_OAUTH_CLIENT_ID`: The client ID to use when connecting to the identity provider.
* `REGISTRY_OAUTH_CLIENT_SECRET`: The client secret to use when connecting to the identity provider.
* `REGISTRY_OAUTH_CLIENT_SCOPE`: The scope to request when redirecting to the identity provider.

### Storage

The persisted data for `cratery` is:
* An sqlite database,
* The index git repository,
* The actual crates packages and metadata,
* The generated documentation of stored crates.

By default, all data is stored in a single directory specificed by the `REGISTRY_DATA_DIR` environment variable.
The default value is a `/data` folder, expected to be mounted into the docker container.

The crates data and their generated documentation can be stored on S3 instead.
This is controlled by the following configuration :
* `REGISTRY_STORAGE`: Either `fs` (default) to store in the `REGISTRY_DATA_DIR` folder or `s3` to store on an S3 bucket.
* `REGISTRY_STORAGE_TIMEOUT`: Timeout (in milli-seconds) to use when interacting with the storage, defaults to 3000
* `REGISTRY_S3_URI`: Top-level domain for the S3 service.
* `REGISTRY_S3_REGION`: Sub-domain for the region.
* `REGISTRY_S3_SERVICE`: Can be left empty ; the sub-domain for the S3 service, if any
* `REGISTRY_S3_ACCESS_KEY`: The access key to use
* `REGISTRY_S3_SECRET_KEY`: The secret key to use
* `REGISTRY_S3_BUCKET`: The S3 bucket to use for storage. It will be created if it does not exist.

The domain for the S3 bucket is interpolated as follow (`REGISTRY_S3_SERVICE` can be left empty):
`{REGISTRY_S3_BUCKET}.{REGISTRY_S3_SERVICE}.{REGISTRY_S3_REGION}.{REGISTRY_S3_URI}`.

### Index

The index for the registry is managed as a git repository.
When `cratery` commits to this repository as an author:
* `REGISTRY_GIT_USER_NAME` is the username to use,
* `REGISTRY_GIT_USER_EMAIL` is the email to use.

The git repository for the index can be synchronized with an externally hosted git repository with:
* `REGISTRY_GIT_REMOTE`: The URI to the remote git repository to use. It will be cloned on startup (or changes pulled from if already present).
* `REGISTRY_GIT_REMOTE_SSH_KEY_FILENAME`: path and filename of the SSH key to use to authenticate to the remote host.
* `REGISTRY_GIT_REMOTE_PUSH_CHANGES`: If set to `true`, changes will be automatically pushed to the remote repository to keep the remote in sync.

### Docs generation

When generating the documentation for stored crates:
* `REGISTRY_SELF_LOCAL_NAME` is the name of the registry for Cargo. It should match the name used to upload the crates.

`cratery` will automatically link to `docs.rs` for dependencies on `crates.io`.
Dependencies to crates also hosted on the same `cratery` instance will be recognized using the `REGISTRY_WEB_PUBLIC_URI` value.

External private registries so that documentation can be generated and the dependencies' docs linked against.
This is specified with the following environment variables.
`{index}` is a number that starts at `1` for the first external registry.
* `REGISTRY_EXTERNAL_{index}_NAME`: The name of the registry for Cargo.
* `REGISTRY_EXTERNAL_{index}_INDEX`: The URL to the registry's index
* `REGISTRY_EXTERNAL_{index}_DOCS`: The URL prefix to use for links to documentation for crates on this registry.
* `REGISTRY_EXTERNAL_{index}_LOGIN`: The login that Cargo will use to get crates from the registry.
* `REGISTRY_EXTERNAL_{index}_TOKEN`: The associated token.


## Contributing

Contributions are welcome!

Open a ticket, ask a question or submit a pull request.


## License

This project is licensed under the [MIT license](https://opensource.org/license/mit).