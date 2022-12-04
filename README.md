# NiCacher

A Nix Binary Cache implemented in Rust

## Objectives

Unlike other existing cache solutions ([Cachix](https://www.cachix.org/) and a S3 bucket), this project aims to "mirror" [cache.nixos.org](cache.nixos.org).
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
