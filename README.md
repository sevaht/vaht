# vaht

A stacking Wayland compositor descended from Fluxbox, written in Rust on
[Smithay](https://github.com/Smithay/smithay).

vaht is in design. Nothing runs yet. The design documents are in
[`docs/design/`](docs/design/).

## Development

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first
use. The checks need four cargo extensions:

```console
$ cargo install --locked cargo-nextest cargo-deny cargo-machete cargo-llvm-cov
```

Run every check (formatting, lints, tests, dependency audit), only the
documentation checks, or the tests with a coverage report:

```console
$ ./checks
$ ./checks --docs
$ ./checks --coverage
```

Browse the generated documentation, private items included:

```console
$ cargo doc --workspace --document-private-items --open
```

## License

[The Unlicense](LICENSE): public domain.
