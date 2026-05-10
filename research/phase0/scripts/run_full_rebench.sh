#!/usr/bin/env bash
# Pull all Tier-1 candidates then run the bench harness over them.
# Designed to be backgrounded — writes structured progress to a log file.
#
# Outputs:
#   results/rebench_progress.log   timestamped per-step status
#   results/bench_db.sqlite        bench rows appended by bench_ollama.py

set -uo pipefail

cd "$(dirname "$0")/.."
LOG=results/rebench_progress.log
mkdir -p results

log() { echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG"; }

TAGS=(
  "qwen3.5:4b-q4_K_M"
  "qwen3.5:9b-q4_K_M"
  "qwen3:4b-instruct-2507-q4_K_M"
  "hf.co/alfredplpl/llm-jp-3-3.7b-instruct-gguf:Q4_K_M"
)

log "=== Phase-0 rebench start ==="
log "Pulling ${#TAGS[@]} candidates (existing pulls skipped automatically by ollama)"

for tag in "${TAGS[@]}"; do
  if ollama list | awk 'NR>1 {print $1}' | grep -Fxq "$tag"; then
    log "[skip-pull] $tag already present"
    continue
  fi
  log "[pull] $tag"
  ollama pull "$tag" >> "$LOG" 2>&1
  rc=$?
  if [[ $rc -ne 0 ]]; then
    log "[pull-FAIL] $tag exit=$rc — continuing with remaining"
  fi
done

log "=== Pull phase done; starting bench (tier1 = Qwen3.5×2 + Qwen3 + LLM-jp) ==="
bash scripts/rebench_4b_candidates.sh tier1 2>&1 | tee -a "$LOG"

log "=== Bench done. Aggregating ==="
python aggregate.py 2>&1 | tee -a "$LOG"

log "=== ALL DONE ==="
