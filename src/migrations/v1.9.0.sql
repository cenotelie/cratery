-- recreate DocGenJob

DROP INDEX IndexDocGenJob;
DROP TABLE DocGenJob;

CREATE TABLE DocGenJob (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    target TEXT NOT NULL,
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



-- create new PackageVersionDocs table

CREATE TABLE PackageVersionDocs (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    target TEXT NOT NULL,
    isAttempted BOOLEAN NOT NULL,
    isPresent BOOLEAN NOT NULL
);

CREATE INDEX IndexPackageVersionDocs ON PackageVersionDocs(package);

-- -- copy the data into PackageVersionDocs
-- INSERT INTO PackageVersionDocs (package, version, target, isAttempted, isPresent)
-- SELECT package, version, 'x86_64-unknown-linux-gnu', docGenAttempted AS isAttempted, hasDocs AS isPresent
-- FROM PackageVersion;



-- drop columns from PackageVersion

CREATE TABLE PackageVersionNew (
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

INSERT INTO PackageVersionNew (package, version, description, upload, uploadedBy, yanked, downloadCount, downloads, depsLastCheck, depsHasOutdated, depsHasCVEs)
SELECT package, version, description, upload, uploadedBy, yanked, downloadCount, downloads, depsLastCheck, depsHasOutdated, depsHasCVEs
FROM PackageVersion;

DROP INDEX IndexPackageVersion;
DROP TABLE PackageVersion;
ALTER TABLE PackageVersionNew RENAME TO PackageVersion;
CREATE INDEX IndexPackageVersion ON PackageVersion(package);
