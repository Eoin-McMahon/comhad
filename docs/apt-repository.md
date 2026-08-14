# The comhad apt repository

This page is for anyone who would rather read what `install.sh` does before
running it. It covers what gets written to your system, what the signing key is,
how to verify it, and how to remove all of it again.

The repository is hosted on GitHub Pages at
<https://eoin-mcmahon.github.io/comhad> and is built by the
[release workflow](../.github/workflows/tag.yml) on every tag. Packages are
published for amd64 and arm64, for Debian 12 or newer and Ubuntu 22.04 or newer.

## What the installer writes

`install.sh` ([source](../.github/pages/install.sh)) touches three paths and
nothing else:

| Path | Contents |
| --- | --- |
| `/etc/apt/keyrings/comhad.gpg` | the public signing key, mode `0644` |
| `/etc/apt/sources.list.d/comhad.list` | one `deb` line pointing at the repository |
| the `comhad` package | installed with `apt-get install -y comhad` |

It refuses to run as anything but root, refuses to run without `apt-get` or
`curl`, and refuses to run on an architecture it has no package for. The key is
downloaded to a temporary file and only then copied into place, so a transfer
that dies halfway cannot leave a truncated keyring behind and break
`apt-get update`.

## Doing it by hand

The same three steps, written out:

```bash
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL -o /tmp/comhad.gpg https://eoin-mcmahon.github.io/comhad/comhad.gpg
sudo install -m 0644 /tmp/comhad.gpg /etc/apt/keyrings/comhad.gpg
echo "deb [arch=amd64,arm64 signed-by=/etc/apt/keyrings/comhad.gpg] https://eoin-mcmahon.github.io/comhad stable main" \
  | sudo tee /etc/apt/sources.list.d/comhad.list > /dev/null
sudo apt-get update && sudo apt-get install comhad
```

Download to a file first rather than piping `curl` into `tee`: `tee` writes as
the stream arrives, so a failed transfer leaves a partial keyring in place.

`arch=amd64,arm64` matters if you have a foreign architecture enabled, which is
common on desktops that run 32-bit Steam or Wine. Without it, apt asks this
repository for a `binary-i386` index that does not exist and `apt-get update`
reports an error.

## The signing key

`comhad.gpg` is a **public** key. Publishing it is the point: apt uses it to
check the signature on the repository index, which in turn covers the checksums
of the `.deb` files. The private half never leaves the GitHub Actions secret it
is stored in.

```
Comhad Repository Signing Key <eoin.mcmahon.dev@gmail.com>
2241 CE8E D086 4E41 2EF1  A9F8 ED2B 3218 F62A 210B
```

Check that the key you downloaded is that key before you trust it:

```bash
gpg --show-keys --with-fingerprint /etc/apt/keyrings/comhad.gpg
```

`signed-by=` in the `deb` line scopes the key to this repository only. It cannot
vouch for packages from Debian, Ubuntu, or any other third party. This is why the
instructions do not use `apt-key add`, which is deprecated and would have added
the key to a store trusted for every source on the system.

## What you are trusting

Two things, stated plainly, because they are true of every third-party apt
repository and not just this one.

**The key arrives over the same channel as the packages.** Verifying the
fingerprint above against a key you fetched from the same GitHub Pages site
proves the download was not corrupted, not that the site is honest. If this
GitHub account or the signing secret were compromised, an attacker could serve a
new key and matching packages together. The fingerprint is published here, in
git history, so a change to it is at least visible.

**The trust is standing, not one-off.** Once the source list is in place, apt
consults this repository on every `apt-get update`, and installs from it as
root. If you want to grant that for `comhad` alone, pin the origin:

```bash
sudo tee /etc/apt/preferences.d/comhad > /dev/null <<'EOF'
Package: *
Pin: origin eoin-mcmahon.github.io
Pin-Priority: 1

Package: comhad
Pin: origin eoin-mcmahon.github.io
Pin-Priority: 500
EOF
```

That leaves `comhad` installable and upgradable while making every other package
name from this repository lose to your distribution's copy.

If you would rather not add a repository at all, the `.deb` files are attached to
each [GitHub release](https://github.com/Eoin-McMahon/Comhad/releases) and
install with `sudo apt install ./comhad_<version>_<arch>.deb`.

## Removing it

```bash
sudo apt-get remove comhad
sudo rm -f /etc/apt/sources.list.d/comhad.list \
           /etc/apt/keyrings/comhad.gpg \
           /etc/apt/preferences.d/comhad
sudo apt-get update
```
