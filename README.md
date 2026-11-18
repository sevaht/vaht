# vaht

A stacking Wayland compositor descended from Fluxbox, written in Rust on
[Smithay](https://github.com/Smithay/smithay).

vaht is early in development. It runs on the hardware from a virtual
terminal (DRM, libinput, libseat), nested in a window of another
compositor, or headless for tests. The design documents are in
[`docs/design/`](docs/design/), and [`DESIGN.md`](DESIGN.md) is the guide
to them.

`vaht check` validates the configuration in `~/.config/vaht`, and
`vaht check --print-defaults` prints the built-in files.
`vaht import fluxbox --from ~/.fluxbox` converts a Fluxbox setup, and
reports every line it could not convert.

## Installing

`packaging/install.sh` builds vaht and installs it with the files it needs
from the system: the login-screen entry (`wayland-sessions/vaht.desktop`),
the choice of portals (`xdg-desktop-portal/vaht-portals.conf`), the session
target that ties the portals to vaht's session
(`systemd/user/vaht-session.target`), and the screen-share portal's
settings for vaht sessions (`/etc/xdg/xdg-desktop-portal-wlr/vaht`), which
ask which screen or window to share with vaht's own menu.

```console
$ sudo packaging/install.sh                 # into /usr (--prefix to change)
$ packaging/install.sh --user               # ~/.local/bin and ~/.config, no root
$ sudo packaging/install.sh --uninstall     # removes what it installed
```

On Arch Linux, `cd packaging/arch && makepkg -si` builds a package that
installs the same files. vaht needs logind or seatd for the seat, and
`xdg-desktop-portal-wlr` for screen sharing in programs (the package
depends on it); `xdg-desktop-portal-gtk` gives programs file choosers.

Working from this checkout, `./launch.sh` starts the installed vaht with
the logging it sets, after checking that the installed vaht was built from
the commit checked out with no changes since (`vaht --version` names the
commit). If not, it gives the commands to build and install it, and waits;
`./launch.sh --build` builds and installs the package itself, then starts it.

To start vaht, log in on a virtual terminal and run `vaht`, or choose
*vaht* at a display manager's login screen. Ctrl+Alt+F1…F12 switch
terminals. The log is `~/.local/state/vaht/vaht.log`.

## Development

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first
use. The checks need four cargo extensions, and `shellcheck` for the shell
scripts:

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
