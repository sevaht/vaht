#!/usr/bin/env bash
# Installs vaht and the files it needs from the system: the executables (vaht
# and vaht-settings, its settings window), the
# login-screen entry (wayland-sessions/vaht.desktop), the portal choice
# (xdg-desktop-portal/vaht-portals.conf) and the session target
# (systemd/user/vaht-session.target), which ties the portals to the session,
# and the screen-share portal's settings for vaht sessions
# (xdg-desktop-portal-wlr/vaht), which pick with vaht's own menu. Packages call it with --destdir;
# a user without root installs with --user.
#
#   packaging/install.sh                   build, then install into /usr (as root)
#   packaging/install.sh --prefix /usr/local
#   packaging/install.sh --user            build, then install for this user only
#   packaging/install.sh --destdir "$pkgdir" --no-build   (a package's install step)
#   packaging/install.sh --uninstall [--user | --prefix ...]
#
# What it installs, system-wide (PREFIX is /usr unless given):
#   PREFIX/bin/vaht
#   PREFIX/bin/vaht-settings
#   PREFIX/share/applications/vaht-settings.desktop    the settings window, in launchers
#   PREFIX/share/wayland-sessions/vaht.desktop        display managers list it
#   PREFIX/share/xdg-desktop-portal/vaht-portals.conf  portals for XDG_CURRENT_DESKTOP=vaht
#   PREFIX/lib/systemd/user/vaht-session.target        the session, for the service manager
#   /etc/xdg/xdg-desktop-portal-wlr/vaht               screen share picks with vaht's menu
#   PREFIX/share/vaht/styles/vaht.toml                 the default style, to read and copy
#   PREFIX/share/doc/vaht/                             README and design notes
#   PREFIX/share/licenses/vaht/LICENSE
# and for one user (--user):
#   ~/.local/bin/vaht
#   ~/.local/bin/vaht-settings
#   ~/.config/xdg-desktop-portal/vaht-portals.conf
#   ~/.config/systemd/user/vaht-session.target
#   ~/.config/xdg-desktop-portal-wlr/vaht
# (no login-screen entry: display managers read only system directories;
# run vaht from a virtual terminal instead).
#
# vaht's configuration is not installed: without ~/.config/vaht, vaht uses
# its built-in files, and `vaht import fluxbox` converts a Fluxbox setup.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

prefix=/usr
destdir=
mode=system
build=yes
action=install

usage() {
    sed -n '2,34p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ ${#} -gt 0 ]]; do
    case ${1} in
        --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
        --destdir) destdir=${2:?--destdir needs a directory}; shift 2 ;;
        --user) mode=user; shift ;;
        --no-build) build=no; shift ;;
        --uninstall) action=uninstall; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install.sh: unknown option ${1}" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ ${mode} == user ]]; then
    bin_dir=${HOME}/.local/bin
    portal_dir=${XDG_CONFIG_HOME:-${HOME}/.config}/xdg-desktop-portal
    unit_dir=${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user
    share_dir=${XDG_CONFIG_HOME:-${HOME}/.config}/xdg-desktop-portal-wlr
    session_dir=
    application_dir=
    doc_dir=
    styles_dir=
    license_dir=
else
    bin_dir=${destdir}${prefix}/bin
    portal_dir=${destdir}${prefix}/share/xdg-desktop-portal
    unit_dir=${destdir}${prefix}/lib/systemd/user
    # The system's XDG configuration directory, whatever the prefix.
    share_dir=${destdir}/etc/xdg/xdg-desktop-portal-wlr
    session_dir=${destdir}${prefix}/share/wayland-sessions
    application_dir=${destdir}${prefix}/share/applications
    doc_dir=${destdir}${prefix}/share/doc/vaht
    styles_dir=${destdir}${prefix}/share/vaht/styles
    license_dir=${destdir}${prefix}/share/licenses/vaht
fi

# Every file this installs, as "source destination mode".
files=(
    "target/release/vaht ${bin_dir}/vaht 755"
    "target/release/vaht-settings ${bin_dir}/vaht-settings 755"
    "data/vaht-portals.conf ${portal_dir}/vaht-portals.conf 644"
    "data/vaht-session.target ${unit_dir}/vaht-session.target 644"
    "data/xdg-desktop-portal-wlr-vaht ${share_dir}/vaht 644"
)
if [[ -n ${session_dir} ]]; then
    files+=(
        "data/vaht.desktop ${session_dir}/vaht.desktop 644"
        "data/styles/vaht.toml ${styles_dir}/vaht.toml 644"
        "data/vaht-settings.desktop ${application_dir}/vaht-settings.desktop 644"
        "README.md ${doc_dir}/README.md 644"
        "DESIGN.md ${doc_dir}/DESIGN.md 644"
        "LICENSE ${license_dir}/LICENSE 644"
    )
fi

if [[ ${action} == uninstall ]]; then
    for entry in "${files[@]}"; do
        read -r _source destination _mode <<<"${entry}"
        if [[ -e ${destination} ]]; then
            rm -v -- "${destination}"
        fi
    done
    for directory in "${doc_dir}" "${license_dir}" "${styles_dir}"; do
        [[ -n ${directory} && -d ${directory} ]] && rmdir --ignore-fail-on-non-empty -- "${directory}"
    done
    exit 0
fi

if [[ ${build} == yes ]]; then
    cargo build --release --locked -p vaht -p vaht-settings
fi
for program in vaht vaht-settings; do
    if [[ ! -x target/release/${program} ]]; then
        echo "install.sh: target/release/${program} is not built; run without --no-build" >&2
        exit 1
    fi
done

for entry in "${files[@]}"; do
    read -r source destination file_mode <<<"${entry}"
    install -Dm"${file_mode}" -- "${source}" "${destination}"
    echo "installed ${destination}"
done

if [[ ${mode} == user && ":${PATH}:" != *":${bin_dir}:"* ]]; then
    echo "note: ${bin_dir} is not in PATH; add it to run vaht by name" >&2
fi
