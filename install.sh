#!/bin/sh
# One-command installer for comhad on Debian and Ubuntu. Adds the signed apt
# repository and installs the package:
#
#   curl -fsSL https://eoin-mcmahon.github.io/comhad/install.sh | sudo bash
#
# What this writes, the signing key's fingerprint, and the same steps by hand,
# for anyone who would rather not pipe a script into a shell:
# https://github.com/Eoin-McMahon/Comhad/blob/master/docs/apt-repository.md
set -eu

repo_url=https://eoin-mcmahon.github.io/comhad
keyring=/etc/apt/keyrings/comhad.gpg
source_list=/etc/apt/sources.list.d/comhad.list

die() {
    echo "install.sh: $1" >&2
    exit 1
}

[ "$(id -u)" = 0 ] || die "must run as root: curl -fsSL $repo_url/install.sh | sudo bash"
command -v apt-get > /dev/null 2>&1 || die "only Debian and Ubuntu are supported here. Other options: $repo_url"
command -v curl > /dev/null 2>&1 || die "curl is required"

arch=$(dpkg --print-architecture)
case "$arch" in
    amd64 | arm64) ;;
    *) die "no comhad package for $arch. Build it instead with: cargo install comhad" ;;
esac

echo "==> Adding the comhad apt repository"
install -d -m 0755 /etc/apt/keyrings
# Download to a temporary file first, so a failed transfer cannot leave a
# truncated keyring behind and break apt-get update.
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$repo_url/comhad.gpg" -o "$tmp"
cat "$tmp" > "$keyring"
chmod 0644 "$keyring"
# Restrict the architectures, so apt does not ask this repository for an index
# it never publishes on hosts with a foreign architecture enabled.
echo "deb [arch=amd64,arm64 signed-by=$keyring] $repo_url stable main" > "$source_list"

echo "==> Updating package lists"
apt-get update

echo "==> Installing comhad"
DEBIAN_FRONTEND=noninteractive apt-get install -y comhad

echo
echo "comhad $(dpkg-query -W -f='${Version}' comhad) installed. Run it with: comhad"
