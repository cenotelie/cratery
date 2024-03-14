ALTER TABLE PackageVersion
    ADD COLUMN docGenAttempted BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE PackageVersion SET docGenAttempted = TRUE WHERE hasDocs = TRUE;
