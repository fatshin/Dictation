# System Audio Capture Plan (PLAUD AI-style meeting transcription)

Investigation-first plan for capturing the PC's audio output (speaker
output / loopback) **in addition to** the microphone input, so Zoom /
Teams / Meet / browser-video audio can be transcribed alongside the
local speaker. Inspired by PLAUD AI's hardware recorder approach but
implemented entirely software-side, preserving the on-device privacy
guarantee.

**Status**: Planning only. No implementation. Outcome of this doc =
a go/no-go decision plus a concrete W-series-style phase plan if go.

## Motivation

The current Dictation pipeline records only the local microphone.
Use-cases that fall outside it:

- Recording a meeting where the remote participant's voice goes to
  the speakers, not the mic.
- Transcribing a YouTube / Vimeo / lecture video you're watching.
- Capturing a podcast or interview audio you're playing back.
- Recording phone calls routed through the Mac (FaceTime audio,
  WhatsApp desktop voice).

PLAUD AI sells a $159-179 USB-C / Lightning microphone that splits
mic + system audio at the hardware level and uploads to their cloud
for transcription. Our equivalent should:

1. Capture system audio (loopback) **and** mic **simultaneously**.
2. Keep both streams **separate** through the ASR pipeline so the
   transcript labels each utterance with its source.
3. Stay **fully local** (CLAUDE.md outbound-network ban applies).
4. Be **opt-in** with explicit consent (legal + privacy).

## Feasibility — does the OS expose what we need?

### macOS

**Until macOS 13**: No public API for system audio loopback. The only
options were:
- Virtual audio drivers: BlackHole, Soundflower, Loopback by Rogue
  Amoeba ($129). User installs a kext-style driver that exposes an
  output device which loops back as an input device. Works but adds
  a third-party install dependency.
- Tap into Core Audio HAL with private APIs (App Store rejection).

**macOS 14.4+** (released 2024-03-07): `ScreenCaptureKit` (introduced
macOS 12.3 for video) gained an **Audio Capture** path that captures
system audio without a virtual driver. Permission gated under
**Screen Recording** in System Settings → Privacy & Security. We can
exclude the Dictation app's own audio output to avoid feedback loops.

API entry point: `SCStream` configured with
`SCStreamConfiguration.capturesAudio = YES` and
`SCContentFilter` excluding the Dictation bundle ID.

Rust bindings:
- `screencapturekit` crate (svtlabs) — wraps ScreenCaptureKit via
  objc2. Last release reviewed: actively maintained as of 2026-05.
  https://github.com/svtlabs/screencapturekit-rs
- Manual `objc2` calls — more work but full control.

Verdict: **macOS 14.4 minimum target buys us a public API**. Pre-14.4
fallback would require a virtual-driver install flow we don't want
to ship. Set minimum macOS version to 14.4 if we ship this feature;
14.0-14.3 users fall back to mic-only.

### Windows

**WASAPI loopback** is a first-party, public, stable API. Has been
shipping since Windows Vista (2007). Activates an `IAudioClient` on
a render endpoint with `AUDCLNT_STREAMFLAGS_LOOPBACK`. No permission
prompt, no driver install.

Process-specific loopback (capture only one app's audio) is newer:
**Windows 11 22H2+** offers a process loopback API via
`ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_PARAMS`
filtering by process ID. On older Windows the loopback is the whole
endpoint mix.

Rust bindings:
- `cpal` (current dependency): exposes WASAPI loopback through
  `Host::default_output_device()` + a loopback input variant. Tested
  patterns exist. https://github.com/RustAudio/cpal/issues/688
- `windows` crate (Microsoft official): full WASAPI access if cpal
  falls short.

Verdict: **WASAPI loopback is the cleanest path.** No third-party
deps, no permission prompts, works back to Windows 10. Process
filtering only on Windows 11 22H2+; on older OS we capture the full
output mix and rely on the user to mute apps they don't want
recorded.

### Cross-platform abstraction

The two implementations are unavoidably different. The abstraction
layer should be at the **audio source enumeration** level:

```rust
// src-tauri/src/audio/sources.rs (sketch)
pub enum AudioSource {
    Microphone(MicConfig),
    SystemLoopback(LoopbackConfig),
}

pub trait AudioSourceCapture: Send {
    fn start(&mut self) -> Result<Receiver<AudioChunk>>;
    fn stop(&mut self);
}

#[cfg(target_os = "macos")]   mod macos_sckit;
#[cfg(target_os = "windows")] mod windows_wasapi;
```

The existing `audio/` module captures only the mic. The new module
sits alongside, sharing the resampler and ring-buffer infrastructure.

## Risk: legal + ethical

This is the dominant risk, not the engineering.

| Jurisdiction | Stance on recording calls without all-parties consent |
|---|---|
| Japan | One-party consent (the user themselves) is generally lawful for personal use. Publishing or coercive use of secret recordings is separately regulated. |
| US — federal + 38 states | One-party consent OK. |
| US — 12 states ("all-party": CA, FL, IL, MA, MD, MT, NH, NV, PA, WA, others) | Recording requires consent of every participant. Civil + criminal liability for violations. |
| EU (GDPR) | Lawful basis required. "Legitimate interest" of the recorder must be balanced against the data subject's expectations. Workplace policies often prohibit. |
| UK | One-party consent for personal use; business use needs disclosure under DPA 2018. |

Implications for the product:

- **Default OFF**. Loopback capture must be opt-in per recording, not
  a persistent setting that runs silently.
- **Visible recording state**. The macOS menu-bar / Windows tray
  icon must change colour/state when loopback is active. No silent
  recording paths.
- **First-use disclaimer**. A dismissible-once dialog stating: "You
  are responsible for the legality of recording other parties.
  Check local law and meeting consent."
- **Audit log**. The encrypted history shows that a session
  included loopback audio (not just mic). Helpful for the user's
  own review of what they captured.

The Plaud-style "always-on background recorder" is **out of scope**
for v1 of this feature; the explicit-per-session model is closer to
the device's privacy posture in CLAUDE.md.

Service-of-Terms risk:
- Zoom shows a "this meeting is being recorded" indicator to all
  participants when Zoom's own recording API is used. Our capture
  bypasses Zoom's notification because we read at the OS level. The
  user is the one with disclosure responsibility, not the app.
- Microsoft Teams and Google Meet behave similarly — our capture is
  invisible to other participants. Make the user understand this.

## Phase decomposition

### S0 — Feasibility spike (3-5 days)

**Goal**: prove the two OS APIs work end-to-end in a throwaway
binary before committing to the real implementation.

- macOS spike: write a 200-line Swift sample that captures 10 s of
  system audio via ScreenCaptureKit, dumps to WAV. Then port to
  `screencapturekit-rs` and verify the same WAV output.
- Windows spike: write a 200-line Rust sample using `cpal` or
  `windows` WASAPI loopback, dump 10 s to WAV.
- Test with Zoom, Teams, Meet, YouTube, Apple Music to confirm each
  source captures cleanly without feedback or clipping.
- Document sample rates and channel layouts each API delivers so the
  resampler stage has stable specs.

**Exit**: two WAVs (one per OS) containing recognisable system
audio from Zoom; document recorded sample rate, channel count, and
bit depth.

### S1 — Dual-source capture in the real app (1-2 weeks)

**Goal**: the user toggles "Include system audio" in the Settings
panel; from then on `start_dictation` opens **two** streams (mic +
loopback) and `stop_dictation` returns both buffers.

- New `audio/sources.rs` with the cross-platform trait above.
- `AsrState.audio` becomes a `Vec<Box<dyn AudioSourceCapture>>`.
- Two ring buffers (`rtrb::Producer/Consumer` pairs) so each stream
  isolates back-pressure.
- Resampling pipeline: both streams to 16 kHz mono f32 (whisper's
  expected input format). Mix down to mono per stream; do not mix
  the two streams together — we need them separate.
- Storage: encrypted `transcripts.audio_chunk` row per stream
  (existing schema may need a `source: "mic" | "loopback"` column).

**Exit**: a single 60 s recording with both mic and loopback active
produces two parsed audio buffers of equal duration with
sample-accurate timestamps.

### S2 — Per-stream transcription + merge (1 week)

**Goal**: each stream is transcribed independently, then the two
transcripts are merged into a single time-ordered conversation log
with speaker labels.

- Run `whisper-rs` twice (one model load, two `create_state()`
  calls). Latency cost: ~2x mic-only mode; acceptable for the
  meeting use-case (latency is less critical than for live
  dictation).
- Per-segment timestamp from Whisper (`get_segment_t0`, `get_segment_t1`).
- Merge: time-sort all segments from both streams; tag each with
  `speaker: "mic" | "loopback"`. The frontend renders the merged
  view.
- Optional: speaker diarisation **within** the loopback stream
  (separating multiple remote speakers) is **out of scope for v1**
  because pyannote / whisperX add 200 MB+ of models and break the
  Whisper-only mode latency budget. Document as v2 follow-up.

**Exit**: a 5-minute Zoom call (one local + one remote speaker)
produces a transcript whose segments are correctly attributed to
either side with timestamps that match the call recording.

### S3 — UI + consent + recording-state visibility (1 week)

**Goal**: the user clearly knows when loopback is active, can scope
it per session, and acknowledges the legal disclaimer once.

- Settings → 一般 → new toggle: "会議モード (システム音声も録音)".
  Off by default.
- Menu-bar icon (macOS) / tray icon (Windows): distinct colour when
  loopback is active vs mic-only.
- First-use modal: legal disclaimer + checkbox "I have responsibility
  for the legality of recording other parties". Click-through is
  required exactly once.
- macOS Screen Recording permission prompt flow on first use of the
  feature. Reuse the existing AX-permission UX scaffolding.
- Per-session "what was recorded" indicator in the history view:
  audio-source icons next to each transcript row.

**Exit**: a tester turns on the feature, sees the consent dialog,
grants Screen Recording permission, records a 5-minute meeting,
sees both speakers in the history view.

### S4 — LLM templates for meeting output (3-5 days)

**Goal**: when the rewrite step runs over a dual-source transcript,
the templates produce useful artifacts (meeting minutes, action
items) instead of a single keigo-rewritten blob.

- New built-in prompt template: `meeting_minutes_ja` and
  `meeting_minutes_en`. Input: the merged transcript with
  speaker labels. Output: a structured summary (議事録形式).
- Add `action_items_ja` / `action_items_en` template that extracts
  TODOs with the speaker attribution.
- Existing `ja_keigo` template should silently skip when input has
  speaker labels (we don't want to rewrite the meeting transcript
  itself; we want to summarise it).
- Settings UI: when the transcript came from dual-source, the
  Rewrite-button dropdown filters to meeting-related templates by
  default.

**Exit**: a 10-minute test meeting produces a ja_keigo-style
議事録 with action items, attributed to the right speakers.

### S5 — File-import fast path (3-5 days)

Folds neatly into ROADMAP Phase 3 ("Meeting and long-form").

- "Import audio file" menu item: pick a `.m4a` / `.wav` / `.mp3`,
  decode (use `symphonia` or `ffmpeg-next`), transcribe through
  the existing whisper pipeline, run the meeting templates.
- Useful when the user recorded a meeting with their phone or with
  Zoom's own cloud recording and wants to feed that into Dictation
  for the rewrite.

**Exit**: dropping a 1-hour meeting M4A on the app produces a
transcript + summary within a reasonable time (<2× audio duration
on M-series).

## Open decisions

1. **Minimum macOS version**. Bumping to 14.4 cuts off macOS 13
   users. Acceptable, but document the support matrix.
2. **Process-specific Windows loopback or whole-output**. Process
   filtering is Windows 11 22H2+; on older OS we capture full output
   and rely on the user. Pick one and document.
3. **Disclaimer wording**. Legal review needed before shipping the
   text. v1 wording should err on the side of "your responsibility
   to comply with local law."
4. **Storage default**. Encrypted audio retention: keep for 7 days,
   30 days, or until manually deleted? Defaults influence privacy
   posture heavily.
5. **Diarisation timing**. v1 = none, v2 = pyannote. Or skip
   diarisation entirely and rely on the OS-level source split
   (mic vs loopback) since that already covers the most common case
   (1:1 calls).

## Risk register

| ID | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| SR-1 | macOS Screen Recording permission has poor UX (per-session prompt) | Med | Med | The "TCC.framework" persistence keeps the grant across sessions; verify in S0 |
| SR-2 | Windows loopback captures Dictation's own audio if the app emits sound | Low | Low | Set up `AUDCLNT_STREAMFLAGS_LOOPBACK` with `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`; on macOS use `SCContentFilter` to exclude self |
| SR-3 | Zoom/Teams ship "anti-recording" measures that block OS-level loopback | Low | High | If it happens, document and remain mic-only for that app. Unlikely because the OS APIs are first-party and don't know what app is running. |
| SR-4 | Legal exposure for the project from users recording without consent | Med | High | Aggressive disclaimer + opt-in flow + visible recording state; not the project's first-line liability but the user's responsibility |
| SR-5 | Audio latency between mic and loopback streams is not sample-accurate | Med | Med | Both streams use system clock; document the worst observed skew in S0; if >100 ms, add a calibration tone procedure |
| SR-6 | Whisper running on two streams in parallel doubles RAM and CPU | High | Med | Sequential rather than parallel transcription for memory-constrained devices; bench on 16 GB target |
| SR-7 | Encrypted audio storage bloats the DB; SQLCipher 4096-byte page size suboptimal for blobs | Med | Med | Store audio outside DB as encrypted file + DB pointer; key still in keychain/DPAPI |

## Honest schedule

| Phase | Effort |
|---|---|
| S0 spike | 3-5 d |
| S1 dual-source capture | 1-2 wk |
| S2 transcription + merge | 1 wk |
| S3 UI + consent | 1 wk |
| S4 meeting templates | 3-5 d |
| S5 file import (overlaps ROADMAP Phase 3) | 3-5 d |
| **Total** | **5-7 wk** for one person |

This is a substantial feature — comparable to the original Phase 1b.
It should not block Phase 1b Windows parity; instead it slots into
**ROADMAP Phase 3** ("Meeting and long-form"), after the v1 ship.

## Recommendation

**Go**, but **after v1.0** ships on macOS and Windows. The feature
genuinely differentiates Dictation against pure dictation tools
(Superwhisper, MacWhisper) and squarely targets the audience listed
in CLAUDE.md (executives, lawyers, doctors who attend confidential
meetings and don't want cloud transcription).

Sequencing:
1. Ship v1.0 (Phase 1a + 1b + Phase 4 distribution) without this.
2. S0 spike in parallel with Phase 1b W3-W4 work — same person
   shouldn't be doing both simultaneously, but a 1-week S0 fits in a
   slack week.
3. S1-S5 as the headline feature of v1.1 / v1.2.

## References

- `docs/ROADMAP.md` § Phase 3 (Meeting and long-form) — natural home
- `docs/ARCHITECTURE.md` § audio pipeline — needs the dual-source
  abstraction
- Apple ScreenCaptureKit Audio:
  https://developer.apple.com/documentation/screencapturekit/capturing_screen_content_in_macos
- WASAPI Loopback Recording:
  https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording
- screencapturekit-rs: https://github.com/svtlabs/screencapturekit-rs
- cpal WASAPI loopback issue: https://github.com/RustAudio/cpal/issues/688
- PLAUD AI (product reference, not used):
  https://www.plaud.ai/
