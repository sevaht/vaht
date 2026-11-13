# vaht

A stacking Wayland compositor descended from Fluxbox, written in Rust on
[Smithay](https://github.com/Smithay/smithay).

vaht is early in development. It runs nested in a window of another
compositor (or headless, for tests), but not yet on its own on real
hardware. The design documents are in [`docs/design/`](docs/design/), and
[`DESIGN.md`](DESIGN.md) is the guide to them.

`vaht check` validates the configuration in `~/.config/vaht`, and
`vaht check --print-defaults` prints the built-in files.
`vaht import fluxbox --from ~/.fluxbox` converts a Fluxbox setup, and
reports every line it could not convert.

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

[MIT](LICENSE), the same license as Smithay.
