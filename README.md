# NiCacher

A Nix Binary Cache implemented in Rust

## Objectives

Unlike other existing cache solutions ([Cachix](https://www.cachix.org/) and a S3 bucket), this project aims to "mirror" upstream caches, such as [cache.nixos.org](cache.nixos.org).
Instead of caching only those requested, a cron job periodically fetches from the upstream.

This project also aims to be able to build derivations with nix and cache them if requested but not avaliable.

## Building

Simply run:
```sh
nix build
```
or
```sh
cargo build
```

### SQLite Database

As sqlx query macros are used to access the SQLite database, extra setup is required for the type-checking capabilities to function.

By default, since the `offline` feature flag is enabled for sqlx and the [sqlx-data.json](./sqlx-data.json) file is checked in, building the crate will function as normal.
However, if there is need for adding new queries, editting existing ones or modifying the schema, one will be required to have a local sqlite database file.

With the `sqlx-cli` utility, this is made simple.

First, either a `DATABASE_FILE` environment variable needs to be set to the location of the SQLite database file, such as `sqlite://cache.db`, or a `.env` file is placed in the repo root.

Creating the database file:
```sh
sqlx database setup
```

Reseting the database file:
```sh
sqlx database reset
```

Updating the `sqlx-data.json` file:
```sh
cargo sqlx prepare
```
