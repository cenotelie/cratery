CREATE TABLE DocGenJob (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    package TEXT NOT NULL REFERENCES Package(name),
    version TEXT NOT NULL,
    targets TEXT NOT NULL,
    state INTEGER NOT NULL,
    queuedOn TIMESTAMP NOT NULL,
    startedOn TIMESTAMP NOT NULL,
    finishedOn TIMESTAMP NOT NULL,
    lastUpdate TIMESTAMP NOT NULL,
    triggerUser INTEGER NOT NULL REFERENCES RegistryUser(id),
    triggerEvent INTEGER NOT NULL,
    output TEXT NOT NULL
);

CREATE INDEX IndexDocGenJob ON DocGenJob (package);
