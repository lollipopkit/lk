#!/usr/bin/env bash
# Reclaim `target/`. Cargo has no stable garbage collector: every time a crate's
# fingerprint changes it writes a *new* artifact into `target/<profile>/deps`
# and leaves the old one there forever. This workspace measured 190GB of
# `target/debug`, of which 41 generations of one archive were 19GB and 3996
# incremental session directories were 65GB.
#
# Everything here is safe to delete at any time. Cargo treats a missing output
# as "not built" and rebuilds it; nothing under `target/` is a source of truth.
# The only cost is build time.
#
#   scripts/prune_target.sh              # incremental dirs + artifacts unused for 14 days
#   scripts/prune_target.sh --days 3     # more aggressive age cutoff
#   scripts/prune_target.sh --keep-incremental
#   scripts/prune_target.sh --dry-run
#
# `cargo clean --gc` does this properly, but it is nightly-only as of 1.90.

set -euo pipefail

DAYS=14
KEEP_INCREMENTAL=0
DRY_RUN=0

while [ $# -gt 0 ]; do
	case "$1" in
	--days)
		DAYS="${2:?--days needs a value}"
		shift 2
		;;
	--keep-incremental)
		KEEP_INCREMENTAL=1
		shift
		;;
	--dry-run | -n)
		DRY_RUN=1
		shift
		;;
	-h | --help)
		sed -n '2,20p' "$0" | sed 's/^# \?//'
		exit 0
		;;
	*)
		echo "unknown argument: $1" >&2
		exit 2
		;;
	esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"

if [ ! -d "$TARGET" ]; then
	echo "no target directory at $TARGET"
	exit 0
fi

before=$(du -sk "$TARGET" | cut -f1)

remove() {
	if [ "$DRY_RUN" = 1 ]; then
		printf '  would remove %s\n' "$1"
	else
		rm -rf -- "$1"
	fi
}

# Incremental state is per-session and rebuilt from scratch; the directories are
# not shared between fingerprints, so a stale one is never read again.
if [ "$KEEP_INCREMENTAL" = 0 ]; then
	count=0
	while IFS= read -r dir; do
		remove "$dir"
		count=$((count + 1))
	done < <(find "$TARGET" -maxdepth 3 -type d -name incremental)
	echo "incremental: $count director$([ "$count" = 1 ] && echo y || echo ies)"
fi

# Age-based, not "keep newest N per crate": cargo hashes the *fingerprint*, not
# a version, so two live artifacts of the same crate (different feature sets,
# different targets) are both current. Access time is what distinguishes a
# generation still being linked from one abandoned by a config change — but
# `relatime` only updates atime once a day, so this uses mtime, which for a
# cargo artifact is its build time.
found=0
while IFS= read -r file; do
	remove "$file"
	found=$((found + 1))
done < <(find "$TARGET" -type f \
	\( -name '*.rlib' -o -name '*.rmeta' -o -name '*.a' -o -name '*.so' -o -name '*.dylib' \) \
	-mtime "+$DAYS")
echo "artifacts older than ${DAYS}d: $found"

if [ "$DRY_RUN" = 1 ]; then
	echo "(dry run — nothing removed)"
	exit 0
fi

after=$(du -sk "$TARGET" | cut -f1)
awk -v b="$before" -v a="$after" \
	'BEGIN { printf "target/: %.1f GB -> %.1f GB (reclaimed %.1f GB)\n", b/1048576, a/1048576, (b-a)/1048576 }'
