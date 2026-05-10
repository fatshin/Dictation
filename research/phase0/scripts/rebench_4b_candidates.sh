#!/usr/bin/env bash
# Re-bench the 16GB-CPU candidate set against the existing ja_keigo /
# en_business / jp_en_mix workloads. Reads tags from ollama_candidates.json,
# feeds each into bench_ollama.py with a stable model_alias for the DB.
#
# Usage:
#   bash research/phase0/scripts/rebench_4b_candidates.sh [tier1]
#
# Tiers:
#   tier1   -> tier_1_primary + tier_1_alternatives  (default)
#   all     -> + tier_2_lighter + fallback_existing
#
# Pre-req:  ollama serve must be running, and each tag must be `ollama pull`ed
# beforehand. The script does NOT auto-pull because pulls are bandwidth-heavy
# and easy to misfire — print the missing pulls instead.

# Intentionally NOT `set -e`: one model crashing the harness must not
# block the remaining candidates. We log each model's exit code instead.
set -uo pipefail

cd "$(dirname "$0")/.."

TIER="${1:-tier1}"
CANDIDATES_JSON="ollama_candidates.json"

if [[ ! -f "$CANDIDATES_JSON" ]]; then
  echo "missing $CANDIDATES_JSON" >&2
  exit 1
fi

case "$TIER" in
  tier1)  KEYS=".tier_1_primary, .tier_1_quality, .tier_1_alternatives" ;;
  all)    KEYS=".tier_1_primary, .tier_1_quality, .tier_1_alternatives, .fallback_existing" ;;
  *)      echo "unknown tier: $TIER (use tier1|all)" >&2; exit 1 ;;
esac

# Build a "alias=tag" line per candidate. POSIX while-read instead of
# mapfile so this also works on macOS' system bash 3.2.
LINES=()
while IFS= read -r line; do
  [[ -n "$line" ]] && LINES+=("$line")
done < <(
  jq -r "[$KEYS] | add | to_entries[] | \"\(.key)=\(.value.ollama_tag)\"" \
    "$CANDIDATES_JSON"
)

if [[ ${#LINES[@]} -eq 0 ]]; then
  echo "no candidates found for tier=$TIER" >&2
  exit 1
fi

echo "=== Phase-0 re-bench: tier=$TIER, ${#LINES[@]} models ==="
for line in "${LINES[@]}"; do
  alias="${line%%=*}"
  tag="${line#*=}"

  # Skip if not pulled.
  if ! ollama list | awk 'NR>1 {print $1}' | grep -Fxq "$tag"; then
    echo "  [skip] $alias ($tag) — not pulled. Run:  ollama pull $tag"
    continue
  fi

  echo "  [bench] $alias ($tag)"
  python bench_ollama.py \
    --model-alias "$alias" \
    --ollama-model "$tag" \
    --runs 3
  rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "  [bench-FAIL] $alias exit=$rc — continuing"
  fi
done

echo "=== done. aggregate with: python aggregate.py ==="
