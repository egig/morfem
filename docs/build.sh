#!/bin/sh
# Regenerate llms-full.txt from the doc pages. Run from anywhere; commit the result.
set -e
cd "$(dirname "$0")"
out=llms-full.txt
: > "$out"
for f in index getting-started npm api queries transactions auth-and-limits; do
  # strip the Jekyll front matter (first block between --- lines)
  awk 'NR==1 && $0=="---"{fm=1; next} fm && $0=="---"{fm=0; next} !fm' "$f.md" >> "$out"
  printf '\n\n' >> "$out"
done
