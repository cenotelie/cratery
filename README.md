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

To launch an empty registry using a pre-built docker image, get the latest `docker-compose.yml` file and start it:

```bash
git clone https://github.com/cenotelie/cratery
cd cratery
docker compose up -d
```

Then, connect to [http://localhost/](http://localhost/).
In the default configuration, a Google account must be used.

The first ever user to log in automatically obtains administration rights.
He/she is then responsible to setup an admin team.

Once connected, a token for CLI usage in Cargo can be obtained by going to [http://localhost/webapp/account.html](http://localhost/webapp/account.html) and clicking on the `Create new token` button.
Tokens can be restricted to read access, e.g. for CI purposes.
For publishing crates, a token with write accesses must be obtained.
The name of the token is just a convenience.
On creation, a popup appear with information about how to register this token for Cargo.


## Features

### Authentication

Authentication is handled using OAuth out of the box.
Simply connect your organisation's provider.
On the default configuration, Google is configured as a provider.
This is only appropriate for demonstration purposes.

### Administration

Administrate owners for hosted crates.

![Screenshot of the admin panel for setting a crate's owner](https://raw.githubusercontent.com/cenotelie/cratery/master/docs/capture-owners.png)

### Docs generation

Cratery automatically generates and serves the documentation for published crates.

![Screenshot of a piece of documentation](https://raw.githubusercontent.com/cenotelie/cratery/master/docs/capture-docs.png)

### Dependency analysis

Cratery automatically scans the dependency graph of the latest versions (for each major version) of hosted crates.
Cratery detects outdated direct dependencies and gives the latest version number to use instead.
Cratery also audits the complete dependency graph to find dependencies, direct or indirect, that are affected by vulnerabilities published by the [RustSec group](https://rustsec.org/).

Cratery can send notifications by emails to the crates' owners when a issue is discovered.
Analysis are also performed on-demand on each crate's page.

![Screenshot of warning about outdated dependencies](https://raw.githubusercontent.com/cenotelie/cratery/master/docs/capture-deps-outdated.png)

![Screenshot of warning about vulnerable dependencies](https://raw.githubusercontent.com/cenotelie/cratery/master/docs/capture-deps-cves.png)

### Statistics

Cratery also tracks downloads to give you statistics about the usage of your crates.

![Screenshot of download statistics for a crate](https://raw.githubusercontent.com/cenotelie/cratery/master/docs/capture-crate-stats.png)

## Configuration

Configuration is passed through environment variables.
See [docker-compose.yml](docker-compose.yml) for all values.

### General

* `REGISTRY_WEB_PUBLIC_URI`: The URI at which the registry will be available.
* `REGISTRY_WEB_COOKIE_SECRET`: The secret key for the private cookie set by `cratery` to track connected users.

### Authentication

Authentication on `cratery` is archived with OAuth and configured with the `REGISTRY_OAUTH_*` environment variables.
The [docker-compose.yml](docker-compose.yml) file contains the basic configuration to use Google as the authentication provider.
This is allowed only for `cratery` instances exposed on `localhost` for evaluation and testing purposes.
This configuration must be changed to use your own OAuth identity provider.

* `REGISTRY_OAUTH_LOGIN_URI`: URI to redirect to when attempting to log in.
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

By default, all data is stored in a single directory specified by the `REGISTRY_DATA_DIR` environment variable.
The default value is a `/data` folder, expected to be mounted into the docker container.

The crates data and their generated documentation can be stored on S3 instead.
This is controlled by the following configuration :
* `REGISTRY_STORAGE`: Either `fs` (default) to store in the `REGISTRY_DATA_DIR` folder or `s3` to store on an S3 bucket.
* `REGISTRY_STORAGE_TIMEOUT`: Timeout (in milli-seconds) to use when interacting with the storage, defaults to 3000
* `REGISTRY_S3_URI`: Top-level domain for the S3 service.
* `REGISTRY_S3_REGION`: Sub-domain for the region.
* `REGISTRY_S3_ACCESS_KEY`: The access key to use
* `REGISTRY_S3_SECRET_KEY`: The secret key to use
* `REGISTRY_S3_BUCKET`: The S3 bucket to use for storage. It will be created if it does not exist.

### Index

The index can be served using both the `git` and `sparse` protocols.
Both are activated by default, but can be activated / deactivated as required:
* `REGISTRY_INDEX_PROTOCOL_GIT`, defaults to `true` to activate the `git` "smart" protocol. Any other value deactivates it.
* `REGISTRY_INDEX_PROTOCOL_SPARSE`, defaults to `true` to activate the `sparse` protocol. Any other value deactivates it.

Fetching the index always requires authentication, regardless of the used protocol.

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

### Dependency analysis

When performing dependency analysis, Cratery will access `crates.io` and other external registries.

* `REGISTRY_DEPS_CHECK_PERIOD`: Period in seconds to wait between checking for crates that need to be analyzed.
* `REGISTRY_DEPS_STALE_REGISTRY`: Number of milliseconds after which the local data about an external registry are deemed stale and must be pulled again. Defaults to 60000 (1 minute).
* `REGISTRY_DEPS_STALE_ANALYSIS`: Number of minutes after which the saved analysis for a crate becomes stale. Defaults to 1 day. A negative number deactivates background analysis of crates.
* `REGISTRY_DEPS_NOTIFY_OUTDATED`: Whether to send a notification by email to the owners of a crate when some of its dependencies become outdated, defaults to `false`. To activate, set to `true`.
* `REGISTRY_DEPS_NOTIFY_CVES`: Whether to send a notification by email to the owners of a crate when CVEs are discovered in its dependencies, defaults to `false`. To activate, set to `true`.
* `REGISTRY_EMAIL_SMTP_HOST`: The host for sending mails.
* `REGISTRY_EMAIL_SMTP_PORT`: The port for sending mails.
* `REGISTRY_EMAIL_SMTP_LOGIN`: The login to connect to the SMTP host.
* `REGISTRY_EMAIL_SMTP_PASSWORD`: The password to connect to the SMTP host
* `REGISTRY_EMAIL_SENDER`: The address to use a sender for mails
* `REGISTRY_EMAIL_CC`: The address to always CC for mails

## Contributing

Contributions are welcome!

Open a ticket, ask a question or submit a pull request.


## License

This project is licensed under the [MIT license](LICENSE).
