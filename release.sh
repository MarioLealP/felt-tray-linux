#!/usr/bin/env bash
# Cut a release: bump versions, regenerate .SRCINFO, tag, push.
# The tag push triggers .gitea/workflows/build.yml -> build + publish.
set -euo pipefail
v="${1:?usage: release.sh X.Y.Z  (or X.Y.Z-N for a packaging-only rebuild)}"

case "$v" in
  *-*) pkgver="${v%-*}"; pkgrel="${v#*-}" ;;
  *)   pkgver="$v";      pkgrel=1 ;;
esac

[ -z "$(git status --porcelain)" ] || { echo "working tree dirty"; exit 1; }

# Cargo.toml (only if this is a Cargo project and pkgver changed)
if [ -f Cargo.toml ] && [ "$pkgrel" = 1 ]; then
  sed -i -E "s/^version = \".*\"/version = \"$pkgver\"/" Cargo.toml
  cargo update --workspace --offline 2>/dev/null || cargo update --workspace
fi

sed -i -E "s/^pkgver=.*/pkgver=$pkgver/; s/^pkgrel=.*/pkgrel=$pkgrel/" PKGBUILD
makepkg --printsrcinfo > .SRCINFO

git add -A
git commit -m "release: v$v"
git tag -a "v$v" -m "v$v"
git push origin HEAD
git push origin "v$v"
echo "tagged v$v — watch the build at git.clockmost.com/pkgs/$(basename "$PWD")/actions"
