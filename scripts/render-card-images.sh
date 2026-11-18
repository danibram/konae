set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT
for app in home docs-site; do
  sips -s format png "$root/examples/$app/public/card.svg" --out "$tmpdir/$app.png" >/dev/null
  ffmpeg -v error -y -i "$tmpdir/$app.png" -pix_fmt rgb24 -compression_level 9 "$tmpdir/$app-opaque.png"
  mv "$tmpdir/$app-opaque.png" "$root/examples/$app/public/card.png"
done
