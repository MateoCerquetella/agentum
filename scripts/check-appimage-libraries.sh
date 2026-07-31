#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: $0 <AppImage-or-extracted-AppDir>" >&2
  exit 2
fi
if ! command -v readelf >/dev/null 2>&1; then
  echo "readelf is required to audit AppImage dependencies" >&2
  exit 2
fi

input="$(realpath "$1")"
temporary_directory=""
if [[ -d "$input" ]]; then
  app_dir="$input"
elif [[ -f "$input" ]]; then
  temporary_directory="$(mktemp -d)"
  cleanup() {
    rm -rf -- "$temporary_directory"
  }
  trap cleanup EXIT
  (
    cd "$temporary_directory"
    APPIMAGE_EXTRACT_AND_RUN=1 "$input" --appimage-extract >/dev/null
  )
  app_dir="$temporary_directory/squashfs-root"
else
  echo "AppImage input does not exist: $input" >&2
  exit 2
fi

if [[ ! -d "$app_dir/usr/lib" || ! -d "$app_dir/usr/bin" ]]; then
  echo "invalid AppDir layout: $app_dir" >&2
  exit 1
fi

for pattern in \
  'libwayland-client.so*' \
  'libwayland-cursor.so*' \
  'libwayland-egl.so*' \
  'libwayland-server.so*'; do
  prohibited_path="$(find "$app_dir" -type f -name "$pattern" -print -quit)"
  if [[ -n "$prohibited_path" ]]; then
    echo "prohibited host graphics library bundled in AppImage: ${prohibited_path#"$app_dir"/}" >&2
    exit 1
  fi
done

desktop_binary="$(find "$app_dir/usr/bin" -type f -name agentum-desktop -print -quit)"
sherpa_library="$(find "$app_dir/usr/lib" -type f -name libsherpa-onnx-c-api.so -print -quit)"
onnx_library="$(find "$app_dir/usr/lib" -type f -name libonnxruntime.so -print -quit)"

for required_path in "$desktop_binary" "$sherpa_library" "$onnx_library"; do
  if [[ -z "$required_path" || ! -s "$required_path" ]]; then
    echo "required AppImage binary or speech library is missing" >&2
    exit 1
  fi
done

if ! readelf -d "$desktop_binary" | grep -Fq 'Shared library: [libsherpa-onnx-c-api.so]'; then
  echo "desktop binary does not declare its Sherpa ONNX dependency" >&2
  exit 1
fi
if ! readelf -d "$sherpa_library" | grep -Fq 'Shared library: [libonnxruntime.so]'; then
  echo "Sherpa ONNX library does not declare its ONNX Runtime dependency" >&2
  exit 1
fi

echo "AppImage library audit: coherent host graphics, bundled Sherpa ONNX runtime"
