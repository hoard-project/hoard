#!/bin/sh
# hoard-atomic.sh — Shell implementation of atomic file writer for Hoard
#
# Usage: echo data | hoard-atomic.sh /path/to/target
#        cat payload | hoard-atomic.sh /var/lib/hoard/volumes/app/data.json
#
# Guarantees Hoard will never see a half-written file.
# Requires: mktemp, cat, mv (all POSIX)

set -e

usage() {
    echo "hoard-atomic: write stdin atomically to a file" >&2
    echo "Usage: hoard-atomic <TARGET>" >&2
    echo "Example: echo data | hoard-atomic /var/lib/hoard/volumes/app/data.json" >&2
    exit 1
}

[ $# -eq 1 ] || usage
[ "$1" = "-h" ] || [ "$1" = "--help" ] && usage

TARGET="$1"
DIR=$(dirname "$TARGET")
BASE=$(basename "$TARGET")

# Create temp file in the SAME directory (rename(2) is only atomic within same fs)
TMP="${DIR}/.${BASE}.hoard-tmp.$$"

# Write stdin to temp file, then fsync + atomic rename
cat > "$TMP"
sync "$TMP" 2>/dev/null || true
mv "$TMP" "$TARGET"

# Belt-and-suspenders: fsync the parent directory
# (ext4/xfs journal the rename, but doesn't hurt)
sync "$DIR" 2>/dev/null || true

echo "hoard-atomic: $(stat -c%s "$TARGET" 2>/dev/null || wc -c < "$TARGET") bytes → $TARGET" >&2
