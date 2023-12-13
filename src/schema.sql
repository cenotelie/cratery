CREATE TABLE IF NOT EXISTS SchemaMetadata (
    name TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS SchemaMetadataIndex ON SchemaMetadata(name);

INSERT INTO SchemaMetadata VALUES ('version', '1.0.0');

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
    lastUsed TIMESTAMP NOT NULL
);

CREATE INDEX IndexRegistryUserToken ON RegistryUserToken (user);

CREATE TABLE Package (
    name TEXT NOT NULL PRIMARY KEY,
    lowercase TEXT NOT NULL
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
    hasDocs BOOLEAN NOT NULL
);

CREATE INDEX IndexPackageVersion ON PackageVersion(package);
