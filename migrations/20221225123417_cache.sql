CREATE TABLE cache (
    hash          TEXT     NOT NULL UNIQUE PRIMARY KEY,
    status        INTEGER  NOT NULL,
    last_cached   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_accessed DATETIME
);

CREATE UNIQUE INDEX cache_hash_index ON cache(hash);
CREATE        INDEX cache_time_index ON cache(last_cached, last_accessed);

CREATE TABLE narinfo (
    hash             TEXT    NOT NULL UNIQUE PRIMARY KEY,
    store_path       TEXT    NOT NULL,
    compression      TEXT    NOT NULL,
    file_hash_method TEXT    NOT NULL,
    file_hash        TEXT    NOT NULL,
    file_size        INTEGER NOT NULL,
    nar_hash_method  TEXT    NOT NULL,
    nar_hash         TEXT    NOT NULL,
    nar_size         INTEGER NOT NULL,
    deriver          TEXT,
    system           TEXT,
    refs             TEXT    NOT NULL,
    signature        TEXT,

    upstream_url     TEXT    NOT NULL,

    FOREIGN KEY(hash) REFERENCES cache(hash)
        ON DELETE CASCADE
);

CREATE UNIQUE INDEX narinfo_hash_index      ON narinfo(hash);
CREATE UNIQUE INDEX narinfo_file_hash_index ON narinfo(file_hash);

