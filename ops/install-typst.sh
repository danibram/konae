#!/bin/sh
set -eu

version=0.14.2
case "$(uname -m)" in
  x86_64) digest=a6044cbad2a954deb921167e257e120ac0a16b20339ec01121194ff9d394996d ;;
  aarch64) digest=491b101aa40a3a7ea82a3f8a6232cabb4e6a7e233810082e5ac812d43fdcd47a ;;
  *) printf 'No Typst binary for %s\n' "$(uname -m)" >&2; exit 1 ;;
esac

name="typst-$(uname -m)-unknown-linux-musl"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' 0
archive="$scratch/$name.tar.xz"
curl --fail --silent --show-error --location \
  "https://github.com/typst/typst/releases/download/v$version/$name.tar.xz" -o "$archive"
printf '%s  %s\n' "$digest" "$archive" | sha256sum --check --status
tar -xJf "$archive" -C "$scratch" "$name/typst"
install -m 755 "$scratch/$name/typst" "$1"
