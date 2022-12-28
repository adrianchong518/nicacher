CREATE TABLE IF NOT EXISTS cache (
    hash TEXT NOT NULL UNIQUE PRIMARY KEY,
    store_path TEXT NOT NULL,
    compression TEXT NOT NULL,
    file_hash_method TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    nar_hash_method TEXT NOT NULL,
    nar_hash TEXT NOT NULL,
    nar_size INTEGER NOT NULL,
    deriver TEXT,
    system TEXT,
    refs TEXT NOT NULL,
    signature TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS narinfo_hash_index ON cache(hash);
CREATE UNIQUE INDEX IF NOT EXISTS narinfo_file_hash_index ON cache(file_hash);

CREATE VIEW narinfo_view AS
SELECT
    hash,
    store_path,
    compression,
    file_hash_method,
    file_hash,
    file_size,
    nar_hash_method,
    nar_hash,
    nar_size,
    deriver,
    system,
    refs,
    signature
FROM cache
