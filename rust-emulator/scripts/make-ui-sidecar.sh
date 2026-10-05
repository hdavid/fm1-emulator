#!/usr/bin/env bash
# Make the web editor sidecar for a Felucca-family firmware package:
#
#   scripts/make-ui-sidecar.sh REPO REF FIRMWARE.fwsc
#
# writes FIRMWARE-ui.zip next to FIRMWARE.fwsc (fm1-ui serves it on
# 127.0.0.1:8765) from REPO's web/ at git REF, as the project's own site
# script (web/make_site.py) lays out the editor: editor.html as index.html,
# plus fukiai.ttf and FUKIAI-LICENSE.txt. SOURCE.txt records where it came from.
set -euo pipefail

[ $# -eq 3 ] || { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
REPO="$1" REF="$2" FIRMWARE="$3"
case "$FIRMWARE" in *.fwsc) ;; *) echo "make-ui-sidecar: $FIRMWARE is not a .fwsc" >&2; exit 2 ;; esac
[ -f "$FIRMWARE" ] || { echo "make-ui-sidecar: no $FIRMWARE" >&2; exit 1; }
OUT="$(cd "$(dirname "$FIRMWARE")" && pwd)/$(basename "$FIRMWARE" .fwsc)-ui.zip"
COMMIT="$(git -C "$REPO" rev-parse --verify "$REF^{commit}")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git -C "$REPO" archive "$COMMIT" web | tar -x -C "$WORK"
[ -f "$WORK/web/editor.html" ] || { echo "make-ui-sidecar: $REPO@$REF has no web/editor.html" >&2; exit 1; }
mkdir "$WORK/ui"
cp "$WORK/web/editor.html" "$WORK/ui/index.html"
for f in fukiai.ttf FUKIAI-LICENSE.txt; do
    [ -f "$WORK/web/$f" ] && cp "$WORK/web/$f" "$WORK/ui/$f"
done
URL="$(git -C "$REPO" remote get-url origin 2>/dev/null || echo "$REPO")"
printf 'Web editor for %s\nfrom %s at %s (%s), web/editor.html (GPL-3.0-only)\n' \
    "$(basename "$FIRMWARE")" "$URL" "$REF" "$COMMIT" > "$WORK/ui/SOURCE.txt"
rm -f "$OUT"
(cd "$WORK/ui" && zip -q -X -r "$OUT" .)
echo "$OUT: $(unzip -Z1 "$OUT" | tr '\n' ' ')($REF = ${COMMIT:0:7})"
