#!/usr/bin/env sh
# The installers (deploy/compose.yaml, deploy/k8s.yaml, which setup.sh applies as
# they are) must name the images of the version in Cargo.toml. When they lag, a
# new install runs an old release and never gets its fixes.
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
failed=0

for file in deploy/compose.yaml deploy/k8s.yaml; do
  images=$(grep -oE 'ghcr\.io/thediymaker/obleth-chat/chat(-ui)?:[^ "]+' "$file" || true)
  if [ -z "$images" ]; then
    echo "error: $file names no chat image" >&2
    failed=1
  fi
  for image in $images; do
    if [ "${image##*:}" != "v$version" ]; then
      echo "error: $file has $image, but Cargo.toml is at $version" >&2
      failed=1
    fi
  done
done

ui=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' ui/package.json | head -n1)
if [ "$ui" != "$version" ]; then
  echo "error: ui/package.json is at $ui, but Cargo.toml is at $version" >&2
  failed=1
fi

exit "$failed"
