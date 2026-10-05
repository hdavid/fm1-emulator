#!/usr/bin/env bash
# Disassemble pi32v2 halfwords with JieLi's own objdump (the vendor toolchain,
# Linux x86-64, run in an amd64 Docker container on macOS).
#
#   scripts/pi32-objdump.sh e1c4 0001 13c0 ...       halfwords, hex
#   scripts/pi32-objdump.sh --image APP.bin ADDR N   N halfwords from an extracted app
#                                                    (examples/extract; loads at 0x02000120)
#
# Toolchain: tools/get_toolchain.sh of Jangada/Felucca installs it in ~/.jieli;
# set JIELI_TOOLCHAIN to the real directory (not the 'toolchain' symlink:
# Docker refuses to execute through it).
set -euo pipefail
TC="${JIELI_TOOLCHAIN:-$(ls -d "$HOME"/.jieli/jieli-linux-toolchains-* 2>/dev/null | sort | tail -1)}"
[ -x "$TC/common/bin/objdump" ] || { echo "pi32-objdump: no JieLi toolchain at '$TC'" >&2; exit 1; }
work="$(mktemp -d "${TMPDIR:-/tmp}/pi32dis.XXXXXX")"
trap 'rm -rf "$work"' EXIT

if [ "${1:-}" = "--image" ]; then
    [ $# -eq 4 ] || { echo "usage: $0 --image APP.bin ADDR N" >&2; exit 2; }
    words="$(python3 -c "
import struct,sys
d=open(sys.argv[1],'rb').read(); a=int(sys.argv[2],16); n=int(sys.argv[3]); o=a-0x02000120
print(' '.join('%04x'%struct.unpack_from('<H',d,o+2*i)[0] for i in range(n)))" "$2" "$3" "$4")"
    start="$3"
else
    [ $# -gt 0 ] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
    words="$*"
    start=""
fi
printf '\t.text\n\t.globl q\nq:\n\t.short %s\n' "$(for w in $words; do printf '0x%s,' "$w"; done | sed 's/,$//')" >"$work/q.s"
docker run --rm --platform linux/amd64 -v "$work:/work" -v "$TC:/opt/jieli:ro" -w /work debian:bookworm-slim \
    sh -c "/opt/jieli/common/bin/clang -target pi32v2 -c q.s -o q.o && /opt/jieli/common/bin/objdump -d q.o" |
    grep -E '^\s+[0-9a-f]+:' |
    if [ -n "$start" ]; then
        python3 -c "
import sys,re
base=int(sys.argv[1],16)
for line in sys.stdin:
    m=re.match(r'(\s+)([0-9a-f]+):(.*)',line.rstrip('\n'))
    print('%08x:%s' % (base+int(m.group(2),16), m.group(3)) if m else line, end='\n')" "$start"
    else
        cat
    fi
