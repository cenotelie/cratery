CREATE TABLE RegistryGlobalToken (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    token TEXT NOT NULL,
    lastUsed TIMESTAMP NOT NULL
);
