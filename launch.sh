#!/usr/bin/env bash
# Runs the installed vaht (the Arch package, packaging/arch) from a virtual
# terminal, with the logging below. Edit the settings between runs; other
# arguments are passed on to vaht (for example --no-xwayland).
#
#   ./launch.sh            check the installed vaht is this checkout, then run it
#   ./launch.sh --build    build and install it first if it is not
#
# The check: the installed vaht was built from the commit checked out, with no
# changes since (`vaht --version` names its commit). If not, without --build
# it says how to build and install it, and waits for Enter before starting the
# installed one anyway; with --build it builds and installs the package
# (asking for your password to install) and then starts it.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

# Which events to record, as tracing filter directives. Per-frame detail
# comes from the compositor and backend at trace; "debug" alone also
# records NVIDIA's driver chatter, hundreds of lines a second.
log_filter='info,vaht_compositor::compositor=trace,vaht_compositor::backend=trace,vaht_compositor::toplevels=debug,vaht_compositor::window_manager=debug,vaht_compositor::xwayland=debug'

# Whether events also go to journald: auto or off. The log file
# (~/.local/state/vaht/vaht.log) gets them either way.
journald=off

installed=/usr/bin/vaht

# --build is launch.sh's own; every other argument goes to vaht.
builds=no
vaht_arguments=()
for argument in "${@}"; do
    if [[ ${argument} == --build ]]; then
        builds=yes
    else
        vaht_arguments+=("${argument}")
    fi
done

# Prints why the installed vaht is not this checkout, one reason a line;
# nothing if it is.
uninstalled_changes() {
    if [[ ! -x ${installed} ]]; then
        echo "vaht is not installed"
        return
    fi
    # `vaht --version` prints "vaht 0.1.0 (<commit>[-modified])".
    local built
    built=$("${installed}" --version | sed -n 's/.*(\(.*\)).*/\1/p')
    local checkout
    checkout=$(git rev-parse --short=12 HEAD)
    if [[ -z ${built} || ${built} == unknown ]]; then
        echo "the installed vaht does not say which commit it was built from"
    elif [[ ${built} == *-modified ]]; then
        echo "the installed vaht was built from uncommitted changes (${built})"
    elif [[ ${built} != "${checkout}" ]]; then
        echo "the installed vaht is ${built}; the checkout is ${checkout}"
    fi
    if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
        echo "the checkout has uncommitted changes"
    fi
}

# Builds the package from this checkout and installs it. A vaht installed
# before it was packaged (no package owns it) is removed first, since pacman
# will not install over files it does not own.
build_and_install() {
    if [[ -e ${installed} ]] && ! pacman -Qqo "${installed}" >/dev/null 2>&1; then
        echo "Removing the vaht installed without a package..."
        sudo packaging/install.sh --uninstall
    fi
    (cd packaging/arch && makepkg -fi)
}

reasons=$(uninstalled_changes)
if [[ -n ${reasons} ]]; then
    echo "The installed vaht is not this checkout:"
    while IFS= read -r reason; do
        echo "  - ${reason}"
    done <<<"${reasons}"
    echo
    if [[ ${builds} == yes ]]; then
        build_and_install
    else
        echo "To build and install it (commit first, so it is named by its commit):"
        echo "  ./launch.sh --build"
        echo "or by hand: cd $(pwd)/packaging/arch && makepkg -fi"
        echo
        if [[ -t 0 ]]; then
            read -r -p "Press Enter to start the installed vaht anyway, or Ctrl+C to stop. "
        fi
    fi
fi

exec "${installed}" \
    --journald "${journald}" \
    --log "${log_filter}" \
    "${vaht_arguments[@]}"
