CREATE TABLE IF NOT EXISTS narinfo (
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

CREATE UNIQUE INDEX IF NOT EXISTS narinfo_hash_index ON narinfo(hash);
CREATE UNIQUE INDEX IF NOT EXISTS narinfo_file_hash_index ON narinfo(file_hash);
