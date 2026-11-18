set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT
for folder in public public/docs; do
  name=$(echo "$folder" | tr / -)
  sips -s format png "$root/examples/site/$folder/card.svg" --out "$tmpdir/$name.png" >/dev/null
  ffmpeg -v error -y -i "$tmpdir/$name.png" -pix_fmt rgb24 -compression_level 9 "$tmpdir/$name-opaque.png"
  mv "$tmpdir/$name-opaque.png" "$root/examples/site/$folder/card.png"
done
