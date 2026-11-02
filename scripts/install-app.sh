#!/bin/bash
# Install the source bundle; no build system or third-party tools required.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$HOME/Applications/Aperture.app"
mkdir -p "$HOME/Applications"
if [[ -e "$DEST" ]]; then
  # Preserve previous installations, rather than deleting them.
  BACKUPS="$HOME/Library/Application Support/Aperture/launcher-backups"
  mkdir -p "$BACKUPS"
  mv "$DEST" "$BACKUPS/Aperture.before-$(date +%Y%m%d-%H%M%S)-$$.app"
fi
/usr/bin/ditto "$ROOT/macos/Aperture.app" "$DEST"
printf '%s\n' "$ROOT" > "$DEST/Contents/Resources/repo-path"
chmod +x "$DEST/Contents/MacOS/Aperture"
/usr/bin/plutil -lint "$DEST/Contents/Info.plist"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST"
echo "Installed $DEST (repository: $ROOT)"
