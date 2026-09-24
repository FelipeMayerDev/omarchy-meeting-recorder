#!/bin/bash
# Installs Meeting Recorder as a pacman package, from the PKGBUILD in this repository.
#   curl -fsSL https://raw.githubusercontent.com/jankeesvw/omarchy-meeting-recorder/main/install.sh | bash
set -euo pipefail

base=https://raw.githubusercontent.com/jankeesvw/omarchy-meeting-recorder/main/packaging/aur
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

curl -fsSLO "$base/PKGBUILD"
curl -fsSLO "$base/omarchy-meeting-recorder.install"
# stdin is this script when it is piped in, so pacman's questions go to the terminal.
makepkg -si --needed </dev/tty
