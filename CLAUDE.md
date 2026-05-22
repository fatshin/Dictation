# モデル最適化 + 3者並列クロスレビュー: グローバル準拠
# 技術調査の事前 grep: ~/Projects/opneclow/wiki/tech/evaluations/ で重複回避
# 詳細: @~/.claude/MODEL_OPTIMIZATION.md / @~/.claude/CLAUDE.md

# Dictation — Local-first AI Dictation App

**Free, fully offline, auditable cross-platform (macOS + Windows) dictation tool with on-device ASR + ローカル LLM 後処理。**

機密素材（会議・医療・法律・社内通信）に耐える privacy-first 設計。本ファイルは本プロジェクト固有の Claude Code 運用規律。

---

## 🔴 大原則 (CRITICAL・全セッション・全エージェント遵守)

### 1. アプリ自体に outbound network call を持たせない

- macOS: `com.apple.security.network.client` entitlement を **付与禁止**
- Tauri: `http:*` capability を **追加禁止**
- どんな"便利機能"も、ネットワーク前提で実装する変更は **REJECTED**
- 例外: モデルファイル初回 DL のみ。それ以外は OS-level firewall で疎通遮断状態でも動作すること

### 2. 透明性・監査可能性を破壊する変更禁止

- ソースコードは MIT 公開前提
- Little Snitch / Wireshark / nettop での疎通検証で「nothing leaves the device」を維持
- telemetry の autonomous 追加は禁止 (将来追加するなら opt-in default off + transcript 内容除外を明記)

### 3. オフライン LLM 一本化を曲げない

- Phase 0 候補: Gemma 4 (E4B/E2B) / Phi-4-mini / Qwen3 4B / SmolLM3 / Llama 3.2 / WhisperKit (ASR)
- クラウド LLM (OpenAI / Anthropic / Gemini) を本体実装で呼び出す変更は禁止
- "性能のために OpenAI 使う" 系の提案は **大原則違反**として保留・CEO 承認必須

### 4. Apple Silicon + Windows ARM/x64 のクロスプラットフォーム対称性

- macOS だけ動けば良い・Windows だけ動けば良い、は不可
- 片側のみで動作する PR は **両 OS の対応計画を併記しない限り REJECTED**

---

## InsForge / Supabase / クラウド BaaS 適用判定: 🔴 REJECTED

opneclow `wiki/tech/evaluations/insforge.md` 評価対象だが、本プロジェクトでは **不採用確定**。

| 理由 | 詳細 |
|---|---|
| Outbound network 禁止 | InsForge は backend ホスト前提・大原則と矛盾 |
| transcript の外部送信不可 | 機密素材の中身を BaaS に送る経路を作らない |
| auditability | クラウド側のコードを監査できない |

代替: SQLCipher + OS-native key storage (Keychain / Secure Enclave / DPAPI / TPM)。

---

## プロジェクト概要

| 項目 | 内容 |
|---|---|
| カテゴリ | クロスプラットフォーム dictation アプリ |
| ターゲット | 機密素材を扱うプロフェッショナル（医師・弁護士・経営層・社内 IT） |
| 競合差別化 | OSS + 完全オフライン + 日本語ビジネス敬体リライト + Win/Mac 対称 |
| ステータス | Pre-PoC（Phase 0 設計レビュー段階） |
| ライセンス | MIT (source) / モデルは個別ライセンス遵守 |

詳細: `README.md` / `README.ja.md` / `docs/ARCHITECTURE.md`

---

## 技術スタック

| Layer | 採用 |
|---|---|
| UI shell | Tauri 2 |
| Frontend | React 19 + TypeScript + Vite 7 + (Zustand) |
| Rust crates | `ort` v2 / `cpal` / `enigo` / `global-hotkey` |
| ASR (Mac) | WhisperKit (Swift sidecar・Apple Neural Engine) |
| ASR (Win) | sherpa-onnx (QNN/DirectML EP) |
| LLM runtime (Phase 0) | `onnxruntime_genai` (Python smoke) |
| LLM runtime (Phase 1+) | `ort` crate v2 + 手動 KV-cache |
| 暗号化 | SQLCipher + Keychain/Secure Enclave (Mac) / DPAPI + TPM (Win) |

依存追加は **package.json / Cargo.toml の dependency 制約レビュー必須** (ライセンス・サイズ・ネットワーク要件)。

---

## ロードマップ

| Phase | スコープ | 状態 |
|---|---|---|
| 0 | LLM 候補 4-6 個ベンチマーク・主候補/フォールバック決定・TTFT 予算検証 | 計画中 |
| 1 | Tauri shell + ASR + LLM rewrite + 暗号化ストレージ + global hotkey + injection | — |
| 2 | 多言語混在・カスタム語彙・per-app トーン切替 | — |
| 3 | 会議ファイル import・長文要約・履歴検索 | — |
| 4 | 署名配布 (notarized DMG / MSIX) + 自動更新 + 公開 | — |

詳細: `docs/ROADMAP.md` / `docs/PHASE0_POC.md`

---

## 実行コマンド

```bash
# 開発サーバ
pnpm dev

# Tauri 起動 (dev)
pnpm tauri dev

# ビルド
pnpm build

# Tauri バンドル
pnpm tauri build

# Phase 0 ベンチマーク (Python sidecar)
cd research/phase0 && uv run benchmark.py
```

---

## クロスレビュー & コミュニケーション

3者並列レビューはグローバル規律通り (`~/.claude/MODEL_OPTIMIZATION.md`) 適用。**ただし**送信内容に注意:

| 観点 | OK | NG |
|---|---|---|
| アーキ図・設計判断・ライセンス調査 | ✅ Codex / Gemini に投げて良い | — |
| 公開リポの差分・PR 内容 | ✅ 投げて良い (MIT 公開コードのため) | — |
| ユーザーの transcript / テスト用音声データ | ❌ | 絶対に Codex/Gemini/Web に送信しない |
| API キー / SQLCipher key / ユーザー設定 | ❌ | 絶対送信しない |

**transcript / 音声 / 復号鍵は本プロジェクトでは Claude Code 自身も読むべきでない**。デバッグで必要なら CEO 承認 + ローカルログでのみ確認。

---

## テスト規律

- ユニット: Rust (`cargo test`) + Vitest (frontend)
- E2E: Tauri test harness + Playwright (web 部分のみ)
- ASR/LLM: Phase 0 ベンチマークが pass しなければ Phase 1 着手不可
- ネットワーク疎通テスト: `nettop` / Little Snitch でアプリ実行中の outbound 0 件を確認
- 80% カバレッジ目標 (グローバル規律通り)

---

## デバッグ・調査時の禁則

- transcript / 音声を **生のまま Web 検索・LLM プロンプトに貼らない**
- バグ再現は仮の文字列・公開可能なサンプル音声で行う
- バグレポート受領時は最初に **PII / 機密が含まれていないか** 確認

---

## モデル選択メモ

候補比較は `wiki/tech/evaluations/` の opneclow 側で管理。本プロジェクト用ローカル評価は `research/phase0/` 配下に保存。`opneclow` cron からの参照は不要。

採用判定マトリクス（各候補に対して評価）:
- TTFT 予算 < ターゲット (Mac M4 / Win Snapdragon X / Win x86_64)
- 日本語ビジネス敬体リライト品質
- INT4 量子化での Perplexity 劣化
- ライセンス（商用配布可否・モデルカード記載）
- ファイルサイズ（初回 DL コスト）

---

## ディレクトリ構造（実装時参考）

```
Dictation/
├── src-tauri/              Rust backend
│   ├── src/{asr,llm,db,keystore,audio,hotkey,inject,network_guard}/
│   └── tauri.conf.json
├── src/                    React frontend
├── sidecars/               Platform-specific binaries (WhisperKit CLI, sherpa-onnx)
├── models/                 Model files (gitignored, downloaded at install)
├── research/phase0/        Phase 0 benchmark scripts
├── scripts/                Build / download / release helpers
└── .github/workflows/      CI + release pipelines
```

---

## 参照ドキュメント

- `README.md` / `README.ja.md` — プロジェクト紹介・設計goals
- `docs/ARCHITECTURE.md` — 詳細アーキテクチャ
- `docs/PHASE0_POC.md` — Phase 0 PoC 計画
- `docs/ROADMAP.md` — フェーズロードマップ
- `LICENSE` — MIT
- `~/.claude/CLAUDE.md` — グローバル規律
- `~/.claude/MODEL_OPTIMIZATION.md` — モデル最適化戦略
- `~/Projects/opneclow/wiki/tech/evaluations/insforge.md` — InsForge 評価（本PJでは REJECTED）


## クロスレビュー記載ルール

レビュー文書に LLM 名称 (Codex/Gemini/Claude 等) を記載しない。レビュワーの**立場・観点**で記述する。
- ✅ 「セキュリティ観点」「パフォーマンス観点」「設計整合性観点」「テスト網羅性観点」
- ❌ 「Codex指摘:」「Gemini指摘:」「Claude観点:」
