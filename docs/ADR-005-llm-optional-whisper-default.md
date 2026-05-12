# ADR-005 — LLM rewrite is optional; Whisper-only is the default

- **Status**: Accepted (2026-05-12)
- **Scope**: (a) Whether a local LLM is a hard requirement of the
  application, and (b) which LLM, if any, is the recommended default.
  Supersedes ADR-004 (which selected `qwen3.5:4b-q4_K_M` as the
  primary local LLM). All other ADR-002 decisions (Ollama as the
  *runtime when present*, HTTP loopback sidecar, SQLCipher storage,
  no outbound network) remain in effect.
- **Supersedes**: ADR-004.
- **Driver**: Two findings on 2026-05-12:
  1. A clean-isolation re-bench reproduces a Qwen3.5 thinking-mode
     failure that **does not depend on the bench host's state** —
     `think: false` is silently ignored by Ollama 0.23.2 for the
     qwen3.5 family. This invalidates ADR-004's selection criterion.
  2. The Whisper-only bypass mode shipped in commit `f1e9ef9`
     removed the architectural premise that an LLM must be present
     for the app to function. The Phase-0 gate "TTFT < 2 500 ms for
     the LLM rewrite" is no longer a *gate on the product*; it is
     now a gate on an *optional path*.

## Context

### Finding 1 — Qwen3.5 thinking is unkillable in Ollama 0.23.2

`qwen3.5:4b-q4_K_M` was selected in ADR-004 on the strength of a
cited 762 ms warm TTFT from
`research/phase0/ollama_candidates.json`. Re-measurement on
2026-05-12 (Ollama 0.23.2, M4 Max, single model resident,
`keep_alive=-1`) consistently produces:

| Metric | Value |
|---|---:|
| Wall-clock first-content latency | 91 000 - 108 000 ms |
| Ollama-reported `load_duration` | 55 000 - 107 000 ms |
| Ollama-reported `prompt_eval_duration` | 280 - 340 ms |
| Ollama-reported `eval_duration` | 1 080 - 1 300 ms |
| Generated text contains `<think>` tag | **No** (with `think: false`) |
| Generated text contains `<think>` tag | **Yes** (with `raw: true` chat template) |

The pattern — first content arrives 90+ seconds after the request,
but prefill and generation each cost under 1.5 seconds — is the
fingerprint of an internal thinking pass that Ollama accounts for as
`load_duration` and does not stream. This was confirmed by:

- Sending `think: false` via `/api/generate` → no `<think>` tag in
  output, but `load_duration ≈ 90 s`.
- Sending `/no_think` as the system message via `/api/chat` →
  identical result.
- Building a custom `Modelfile` with `PARAMETER think false` →
  rejected with `unknown parameter 'think'` (Ollama 0.23.2 Modelfile
  syntax does not accept this).
- Building a `Modelfile` whose `TEMPLATE` skips the thinking turn →
  Ollama's built-in `RENDERER qwen3.5` overrides the template and
  the thinking pass still runs.

Result: Qwen3.5 family models cannot satisfy the 2 500 ms TTFT
hard line under the current Ollama runtime. The previous ADR-004
TTFT figure was either cherry-picked from a non-reproducible warm
run or measured on a setup that bypassed the renderer.

This finding is **systematic, not host-state dependent**.

### Finding 2 — Whisper-only bypass changes what the app needs

Commit `f1e9ef9` (2026-05-12) introduced `app_settings.bypass_llm`,
implemented across the three pipeline entry points (Stop button,
Cmd+Shift+D, fn long-press) via a single `handleTranscriptCompleted`
helper. With `bypass_llm = true`:

- `check_setup` succeeds with only the Whisper model present
  (Ollama is not required).
- The raw Whisper transcript is fed directly to the paste path.
- `whisper_rs::set_initial_prompt` is wired so vocabulary biasing
  (proper nouns, acronyms, style hints) still works.

The end-of-pipeline experience in bypass mode is: hold fn → speak
→ release → paste appears in the focused app. No LLM call. The
ASR latency budget (target < 800 ms warm on `small.bin`, M4) is the
only latency gate.

### Finding 3 — Gemma-4 measurements are inconclusive

The 2026-05-12 session also measured `gemma4:e4b` and `gemma4:e2b`
showing 60-115 s `load_duration` per warm request, contradicting the
original Phase-0 figures of 246 ms and 211 ms. However:

- The bench host had `swap_used = 21 GB`, `pages_free = 72 MB`, and
  1.25 M swap-ins at the time, because the session had previously
  loaded `qwen3.6:latest` (23 GB) and `qwen3-coder:30b` (18 GB).
- The Phase-0 measurements were on Ollama 0.21.0, not 0.23.2.
- Both factors (swap pressure, Ollama version) could explain the
  delta. We have not isolated them.

**We do not have a reproducible Gemma-4 finding.** ADR-004 demoted
Gemma-4 on disk-size grounds (9.6 GB / 7.2 GB on a 16 GB target);
that argument is still correct. But the *speed* argument used to
elevate Qwen3.5 over Gemma-4 in ADR-004 has no surviving evidence
either direction. This is an Open Verification item below.

## Decision

### Architectural

**The LLM rewrite step is optional, not required.** The dictation
pipeline ships as:

```
audio → Whisper → (paste OR Whisper + LLM rewrite → paste)
```

with the user controlling which path via the `bypass_llm` setting.
Default for new installs is **bypass_llm = true** (Whisper-only).

This is implemented; this ADR formally documents the design choice.

### Recommended model (when LLM rewrite is enabled)

**No recommendation today.** Three independent open verification
items must close before we elevate any model to a "recommended"
slot in the setup wizard:

1. Confirm whether Ollama 0.21.0 still produces fast warm TTFT for
   Gemma-4 (rules out swap contamination as the only cause of the
   2026-05-12 regression).
2. Compare Ollama 0.23.2 against llama.cpp directly using the same
   Gemma-4 weights (isolates the runtime variable).
3. Track upstream Ollama for a real Qwen3 thinking-disable knob; if
   one ships, re-test qwen3.5:4b-q4_K_M.

Until then, the LLM rewrite is **opt-in with no default model**.
The Settings UI lists models the user has manually pulled with
`ollama pull`, and warns "no rewrite model has been validated for
your runtime yet."

`PREFERRED_MODELS` in `src/App.tsx` and `REQUIRED_MODELS` in
`src-tauri/src/commands.rs` are kept in their current order for
the optional setup-wizard auto-pull, but the wizard no longer
auto-pulls anything when bypass_llm is the default:

```
PREFERRED_MODELS (opt-in auto-pull order, unchanged from ADR-004):
1. qwen3.5:4b-q4_K_M               (suspended pending Open #3)
2. qwen3:4b-instruct-2507-q4_K_M
3. hf.co/alfredplpl/llm-jp-3-3.7b-instruct-gguf:Q4_K_M
4. gemma4:e4b                      (re-evaluate after Open #1, #2)
5. gemma4:e2b                      (re-evaluate after Open #1, #2)
```

## Product-proposition impact

CLAUDE.md positions Dictation's competitive differentiator as
*"OSS + 完全オフライン + 日本語ビジネス敬体リライト + Win/Mac 対称"*.

Making Whisper-only the default removes "日本語ビジネス敬体リライト"
from the **out-of-box** experience. The honest reframing:

| Differentiator | Status under ADR-005 |
|---|---|
| OSS (MIT) | Unchanged — strong |
| Fully offline | **Strengthened** — Whisper-only mode lets us *guarantee* no LLM call, no Ollama dependency, no model file. The privacy claim becomes provable with `nettop` showing zero flows. |
| 日本語ビジネス敬体リライト | **Moved to opt-in.** Still differentiates against Superwhisper/MacWhisper for the user who turns it on; not present for the user who doesn't. |
| Win/Mac symmetric | Unchanged — Whisper + bypass works on both; LLM-runtime parity is the harder problem and is the subject of `WINDOWS_PLAN.md` W6. |
| **NEW: minimum-trust dictation** | The Whisper-only mode is a genuine new market segment — users (medical, legal, executive, intelligence) who refuse any LLM in the pipeline because LLMs are non-deterministic and harder to audit than a pure ASR system. This is *not* a competitor's positioning. |

The user-facing story shifts from
*"local dictation that also rewrites for you"* to
*"local dictation that can optionally rewrite for you, with rewrite
disabled until you opt in"*.

Marketing copy and the first-run wizard need to be re-authored to
match. This is a Phase 4 (M6 — public download experience) task,
not blocking.

## Consequences

### Benefits

- **Ship today.** The Whisper-only path works on every macOS Apple
  Silicon and Windows x64 machine with 8+ GB RAM, no LLM gate, no
  model-pull wait. We are not blocked by the LLM-runtime situation.
- **The 16 GB-RAM target stops being a constraint.** Whisper +
  small.bin is ~600 MB resident. No swap pressure, no eviction
  thrashing, no 9.6 GB pull.
- **Privacy claim becomes provable.** "No data leaves the device" is
  trivially true when there is no LLM in the pipeline. We add a
  Settings → "Verify locality" button that runs `nettop` (mac) /
  `Get-NetTCPConnection` (Windows) and shows the user the empty
  flow list during a recording.
- **Phase-0 gate honesty.** ADR-002 set TTFT < 2 500 ms as an LLM
  gate; no Phase-0 candidate met it under clean re-measurement.
  Continuing to ship behind that gate would be cargo-cult. The new
  default path has a different and *achievable* gate
  (ASR latency only).
- **Ollama dependency becomes pluggable.** When llama.cpp or mlx-lm
  matures and outperforms Ollama, we swap the LLM runtime without
  affecting most users — they're in bypass mode anyway.

### Costs / risks

- **Loss of out-of-box keigo rewrite.** New users who don't read the
  Settings panel get raw transcription. They may compare us
  unfavourably against Superwhisper (which rewrites by default) and
  miss that the rewrite is one toggle away.
  - Mitigation: first-run wizard explains the bypass default and
    offers a one-click "enable LLM rewrite (downloads ~7 GB)" path.
- **Discoverability of `whisper_initial_prompt`.** The vocabulary
  biasing in bypass mode is a power-user feature; most users won't
  know it exists.
  - Mitigation: pre-populate a "starter pack" of common JP/EN
    technical terms in the placeholder text on first use.
- **Public bench data is now stale.** The Phase-0 report and
  `ollama_candidates.json` recommend Qwen3.5-4B; new contributors
  reading those files will be confused. We need to add a banner
  pointing at this ADR.
  - Mitigation: a one-paragraph "NOTE — superseded by ADR-005" block
    at the top of those two files. Phase-0 docs are kept for the
    audit trail of how we got here.
- **Phase 1b W6 is now uncertain.** WINDOWS_PLAN.md schedules a week
  for Ollama-Windows integration. If the Open Verification items
  below conclude that no Ollama-based path works, W6 may pivot to
  llama.cpp-Windows instead, with its own setup cost.
- **No A/B baseline against ADR-004's claim.** We cannot retroactively
  prove the 2026-05-11 selection was wrong by re-running its bench;
  the data it relied on is no longer reproducible. This ADR rests on
  the thinking-mode finding (which IS reproducible) plus the
  architectural decoupling (which IS shipped).

## Rejected alternatives

- **Pick *some* LLM as the default anyway.** Every measured candidate
  failed under clean re-measurement. Picking one would repeat
  ADR-004's mistake. Wait for the open verifications to close.
- **Hard-require Whisper-only — no LLM rewrite ever.** Over-correction.
  The LLM path adds genuine value for users who want keigo conversion;
  removing it loses a differentiator. Keep it as opt-in.
- **Pin Ollama to 0.21.0 globally.** Tempting because Phase-0 numbers
  came from that version. Rejected without first confirming the
  regression actually exists (Open Verification #1). Pinning a
  one-year-stale runtime to chase one regression is a maintenance
  burden, not a fix.
- **Switch LLM runtime to mlx-lm on macOS.** Apple-only; breaks the
  Win/Mac symmetry that ADR-001 / ADR-002 protected. Acceptable as a
  fast-path on Apple Silicon if/when we have evidence it's
  meaningfully better, but not as the default.
- **Implement thinking-token streaming for Qwen3.5 ourselves.**
  Would let us show progress during the 90 s wait instead of
  appearing frozen. Rejected because a 90 s TTFT is unusable for
  dictation regardless of whether we show progress; fixes the
  symptom not the cause.

## Open verification

(These gate any future "recommended LLM" decision; they do NOT gate
shipping ADR-005's architectural decision.)

1. **Gemma-4 on Ollama 0.21.0 with a clean host.** Boot a freshly
   restarted machine, pull only `gemma4:e2b`, run
   `bench_ollama.py --runs 5 --warmup 1 --only ja_keigo_01`. Compare
   `load_duration` against the original 211 ms figure.
2. **Gemma-4 on llama.cpp direct (Ollama-independent).** Same prompt,
   same weights, same machine, llama.cpp's CLI rather than Ollama's
   HTTP API. Establishes whether the slowdown is Ollama-specific.
3. **Ollama Qwen3 thinking-disable knob.** Track
   github.com/ollama/ollama for any future PR or release that
   exposes a real `disable_thinking=true` flag for qwen3.5. If/when
   one ships, re-bench. The Apache-2.0 license + 2.6 GB footprint
   would make qwen3.5:4b attractive again.
4. **16 GB-RAM target end-to-end Whisper-only test.** On a 16 GB
   Intel Mac and a 16 GB Windows machine, confirm
   `record → transcribe → paste` completes in < 1 500 ms for a 5 s
   utterance. The default-path latency gate.

## Acceptance criteria for shipping under ADR-005

- [x] `bypass_llm` toggle exists in Settings and persists across
      relaunches (shipped in `f1e9ef9`).
- [x] `check_setup` returns `ready = true` when only the Whisper
      model is present (shipped in `f1e9ef9`).
- [x] All three pipeline entry points (Stop button, Cmd+Shift+D,
      fn long-press) honour bypass mode (shipped in `f1e9ef9`).
- [ ] First-run wizard explains the bypass default and offers a
      one-click LLM-rewrite opt-in. **Open** — to be authored.
- [ ] Phase-0 report + ollama_candidates.json carry a banner pointing
      at this ADR. **Open** — small follow-up commit.
- [ ] CLAUDE.md positioning text updated to reflect the new product
      story. **Open** — CEO-level decision, not engineering.
- [ ] README / landing copy reframes the differentiator from "AI
      rewrite" to "minimum-trust local dictation with optional AI
      rewrite". **Open** — Phase 4 work.

## References

- ADR-004: `docs/ADR-004-primary-llm-qwen35-4b.md` — the 2026-05-11
  decision this supersedes, retained as audit trail.
- ADR-002: `docs/ADR-002-runtime-pivot-ollama-gemma4.md` — runtime
  decision (Ollama HTTP sidecar) still in effect when LLM rewrite is
  enabled.
- Phase-0 bench: `research/phase0/results/report.md`,
  `research/phase0/ollama_candidates.json` — will be banner-noted as
  superseded.
- Re-bench harness: `research/phase0/speed_check.py` — added in this
  session for clean-isolation measurement.
- Bypass-mode implementation: commit `f1e9ef9`.
- 2026-05-12 thinking-mode evidence: terminal session showing
  `qwen3.5:4b-q4_K_M` with `think: false` producing
  `load_duration = 96 697 / 107 493 / 55 257 ms` across three warm
  runs with `prompt_eval_duration ≈ 300 ms` in each — wall time is
  not prefill.
- Ollama Qwen3 docs (claims `think: false` works; observation on
  0.23.2 contradicts for qwen3.5):
  https://ollama.com/library/qwen3
- `WINDOWS_PLAN.md` § W6 — Ollama Windows integration may pivot to
  llama.cpp pending Open Verification outcomes.
- `MAC_PACKAGING.md` § M6 — first-run UX needs to incorporate the
  bypass-default messaging.
