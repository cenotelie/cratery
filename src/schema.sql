CREATE TABLE IF NOT EXISTS SchemaMetadata (
    name TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS SchemaMetadataIndex ON SchemaMetadata(name);

INSERT INTO SchemaMetadata VALUES ('version', '1.11.0');

CREATE TABLE RegistryUser (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    isActive BOOLEAN NOT NULL,
    email TEXT NOT NULL,
    login TEXT NOT NULL,
    name TEXT NOT NULL,
    roles TEXT NOT NULL
);

CREATE INDEX IndexRegistryUserByEmail ON RegistryUser (email);
CREATE INDEX IndexRegistryUserByLogin ON RegistryUser (login);

CREATE TABLE RegistryUserToken (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    user INTEGER NOT NULL REFERENCES RegistryUser(id),
    name TEXT NOT NULL,
    token TEXT NOT NULL,
    lastUsed TIMESTAMP NOT NULL,
    canWrite BOOLEAN NOT NULL,
    canAdmin BOOLEAN NOT NULL
);

CREATE INDEX IndexRegistryUserToken ON RegistryUserToken (user);

CREATE TABLE RegistryGlobalToken (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    token TEXT NOT NULL,
    lastUsed TIMESTAMP NOT NULL
);

CREATE TABLE Package (
    name TEXT NOT NULL PRIMARY KEY,
    lowercase TEXT NOT NULL,
    targets TEXT NOT NULL,
    nativeTargets TEXT NOT NULL,
    capabilities TEXT NOT NULL,
    isDeprecated BOOLEAN NOT NULL,
    canOverwrite BOOLEAN NOT NULL
);

CREATE INDEX IndexPackage ON Package (name);

CREATE TABLE PackageOwner (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    owner INTEGER NOT NULL REFERENCES RegistryUser(id)
);

CREATE INDEX IndexPackageOwner ON PackageOwner (package);

CREATE TABLE PackageVersion (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    description TEXT NOT NULL,
    upload TIMESTAMP NOT NULL,
    uploadedBy INTEGER NOT NULL REFERENCES RegistryUser(id),
    yanked BOOLEAN NOT NULL,
    downloadCount INTEGER NOT NULL,
    downloads BLOB,
    depsLastCheck TIMESTAMP NOT NULL,
    depsHasOutdated BOOLEAN NOT NULL,
    depsHasCVEs BOOLEAN NOT NULL
);

CREATE INDEX IndexPackageVersion ON PackageVersion(package);

CREATE TABLE PackageVersionDocs (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    target TEXT NOT NULL,
    isAttempted BOOLEAN NOT NULL,
    isPresent BOOLEAN NOT NULL
);

CREATE INDEX IndexPackageVersionDocs ON PackageVersionDocs(package);

CREATE TABLE DocGenJob (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    target TEXT NOT NULL,
    useNative BOOLEAN NOT NULL,
    capabilities TEXT NOT NULL,
    state INTEGER NOT NULL,
    queuedOn TIMESTAMP NOT NULL,
    startedOn TIMESTAMP NOT NULL,
    finishedOn TIMESTAMP NOT NULL,
    lastUpdate TIMESTAMP NOT NULL,
    triggerUser INTEGER REFERENCES RegistryUser(id),
    triggerEvent INTEGER NOT NULL,
    output TEXT NOT NULL
);

CREATE INDEX IndexDocGenJob ON DocGenJob (package);
