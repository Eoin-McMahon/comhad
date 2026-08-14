#!/usr/bin/env bash
# Regenerate and sign the apt repository indexes for the pool in $1.
#
# Requires dpkg-dev, apt-utils, and a signing key already present in the
# keyring. Set GPG_KEY_ID to the fingerprint to sign with.
set -euo pipefail

repo_dir=${1:?usage: build-apt-repo.sh <repo-dir>}
: "${GPG_KEY_ID:?GPG_KEY_ID is not set}"

suite=stable
component=main
arches=(amd64 arm64)

cd "$repo_dir"

if [ -z "$(find pool -name '*.deb' -print -quit 2>/dev/null)" ]; then
    echo "no .deb files under $repo_dir/pool" >&2
    exit 1
fi

for arch in "${arches[@]}"; do
    dist_dir="dists/$suite/$component/binary-$arch"
    mkdir -p "$dist_dir"
    # --multiversion keeps every version in the index, not just the newest, so
    # older releases stay installable with apt-get install comhad=<version>.
    dpkg-scanpackages --multiversion --arch "$arch" pool > "$dist_dir/Packages"
    gzip -9cn "$dist_dir/Packages" > "$dist_dir/Packages.gz"
done

apt-ftparchive \
    -o "APT::FTPArchive::Release::Origin=comhad" \
    -o "APT::FTPArchive::Release::Label=comhad" \
    -o "APT::FTPArchive::Release::Suite=$suite" \
    -o "APT::FTPArchive::Release::Codename=$suite" \
    -o "APT::FTPArchive::Release::Components=$component" \
    -o "APT::FTPArchive::Release::Architectures=${arches[*]}" \
    release "dists/$suite" > "dists/$suite/Release"

# Both signature forms, because apt clients in the wild still use each.
rm -f "dists/$suite/Release.gpg" "dists/$suite/InRelease"
gpg --batch --yes --local-user "$GPG_KEY_ID" \
    --armor --detach-sign --output "dists/$suite/Release.gpg" "dists/$suite/Release"
gpg --batch --yes --local-user "$GPG_KEY_ID" \
    --clearsign --output "dists/$suite/InRelease" "dists/$suite/Release"
