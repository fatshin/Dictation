# Dictation

**無料・ローカル完結の AI 音声入力アプリ。あなたの声はマシンから出ない。**

macOS と Windows で動作するクロスプラットフォーム音声入力ツール。オンデバイスの音声認識 + ローカル LLM による後処理で構成されます。機微情報を扱う人向け — 会議、医療記録、法務ドラフト、社内コミュニケーションなど、音声をサードパーティクラウドへ送れない状況を前提に設計しています。

> **ステータス**: Pre-PoC。アーキテクチャと Phase 0 計画をレビュー用に公開中。実コードは Phase 0 の Go/No-Go 判定後に投入します。

English: [README.md](README.md)

---

## なぜもう一つ dictation アプリを作るのか

| 既存ツール | 問題 |
|---|---|
| Superwhisper, Wispr Flow, Typeless | サブスク課金、クラウド処理、または検証困難なプライバシー主張 |
| MacWhisper | macOS 専用、LLM 後処理が薄い |
| Whispering (OSS) | 日本語ビジネス敬語のリライトなし、UX が粗い |
| OS 標準 (macOS Dictation, Windows Voice Access) | リライト機能弱い、クロスアプリのホットキー運用なし |

本プロジェクトが狙う空白地帯: **ソースコード監査可能 + 完全オフライン + 日本語ビジネス敬語 + Windows/macOS パリティ**。

---

## 設計方針

1. **永久無料で動く**。サブスクなし、phone-home なし、アカウント必須ではない。
2. **ローカル完結**。ASR と LLM 推論は全てマシン内で実行。デフォルトで外部ネットワーク接続なし。
3. **監査可能**。ソースコード公開。Little Snitch / Wireshark / PacketCapture で「デバイスから何も出ていない」ことを自分で検証可能。
4. **クロスプラットフォーム**。macOS (Apple Silicon) と Windows (x64 / ARM64) を単一コードベースで。
5. **日英第一級対応**。言語混在 dictation、ビジネス敬語リライト、カスタム語彙。

---

## アーキテクチャ (計画)

```
┌───────────────────────────────────────────────────────────────┐
│ Tauri 2 Shell (Rust バックエンド + React/TS フロントエンド)    │
│                                                               │
│  Hotkey ─▶ 音声取得 ─▶ ASR ─▶ LLM リライト ─▶ テキスト挿入     │
│                         │       │                             │
│                Mac: WhisperKit  │ ONNX Runtime GenAI         │
│                Win: sherpa-onnx │ (CoreML / DirectML /       │
│                                 │  QNN / OpenVINO EP)         │
│                                                               │
│  暗号化ローカル DB (SQLCipher + OS 鍵ストア)                    │
│  ネットワーク gard: デフォルトで外部接続なし                    │
└───────────────────────────────────────────────────────────────┘
```

### 技術スタック

| レイヤ | 選定 | 根拠 |
|---|---|---|
| UI シェル | Tauri 2 | バンドル小、Rust バックエンド、クロスプラットフォーム |
| フロントエンド | React + TypeScript + Zustand | 広く知られている、高速反復 |
| LLM ランタイム (Phase 0) | `onnxruntime_genai` Python + ONNX Runtime スモークテスト | 高速反復、レイテンシ予算確立 |
| LLM ランタイム (Phase 1+) | `ort` crate v2 + 手動 KV-cache loop (必要なら C API FFI ラップ) | Rust バックエンド、Phase 0 ゲート通過後 |
| ASR (macOS) | WhisperKit (Swift sidecar) | Apple Neural Engine 加速 |
| ASR (Windows) | sherpa-onnx | QNN/DirectML EP、Whisper large-v3-turbo ONNX |
| 暗号化 | SQLCipher + Keychain/Secure Enclave (Mac)、DPAPI + TPM (Win) | OS ネイティブ鍵ストア |
| ホットキー | `global-hotkey` crate | クロスプラットフォーム |
| テキスト挿入 | `enigo` + プラットフォーム accessibility API | 任意アプリ対応 |
| 音声取得 | `cpal` + lock-free ring buffer | 低遅延 PCM |

### 候補 LLM (Phase 0 でベンチ)

全て小型 (≤4B パラメータ) で INT4 ONNX 量子化済み。コンシューマノートで動作する範囲。

| モデル | サイズ | ライセンス | 備考 |
|---|---|---|---|
| Gemma 4 E4B | 4.5B 実効 | Apache 2.0 | 音声入力ネイティブ (audio_encoder_q4.onnx 同梱)、128K コンテキスト |
| Gemma 4 E2B | 2B 実効 | Apache 2.0 | 軽量版 |
| Phi-4-mini-instruct | 3.8B | MIT | CPU/GPU INT4 RTN バリアント、英語+日本語強い |
| Qwen3 4B Instruct 2507 | 4B | Apache 2.0 (model card で要確認) | 多言語 (日本語含) 強い |
| Llama 3.2 3B | 3B | Llama 3.2 License | フォールバック、Meta 量子化 ONNX あり |
| SmolLM3-3B | 3B | Apache 2.0 | **英語 + 欧州 5 言語のみ、日本語非対応**。英語長文要約のフォールバックとして検討 |

モデルファイルは初回起動時にダウンロード。バイナリには埋め込まず SHA-256 検証あり。

---

## プロジェクト構成 (計画)

```
Dictation/
├── src-tauri/              Rust バックエンド
│   ├── src/
│   │   ├── asr/            ASR 抽象化 (Mac/Win 実装)
│   │   ├── llm/            ONNX Runtime GenAI ラッパー
│   │   ├── db/             SQLCipher + マイグレーション
│   │   ├── keystore/       Keychain / DPAPI
│   │   ├── audio/          Recorder、ring buffer
│   │   ├── hotkey/         グローバルホットキー
│   │   ├── inject/         テキスト挿入 (enigo + AX / UIA)
│   │   └── network_guard/  外向き接続ブロックポリシー
│   └── tauri.conf.json
├── src/                    React フロントエンド (Vite)
├── sidecars/               プラットフォーム固有バイナリ (WhisperKit CLI, sherpa-onnx)
├── models/                 モデルファイル (git 無視、初回DL)
├── research/phase0/        Phase 0 ベンチスクリプト
├── scripts/                ビルド / DL / リリースヘルパ
└── .github/workflows/      CI + リリースパイプライン
```

詳細は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) を参照。

---

## ロードマップ

| Phase | 範囲 | ステータス |
|---|---|---|
| 0 | 技術 PoC — 4〜6 候補 LLM を Mac + Windows でベンチ、本命 + フォールバック選定、TTFT 予算実証 | 計画中 |
| 1 | MVP — Tauri 2 シェル、ASR 統合、LLM リライト、暗号化ローカルストレージ、グローバルホットキー、テキスト挿入 | — |
| 2 | 日英混在処理、カスタム語彙、アプリ別トーン切替 | — |
| 3 | 会議ファイル取込、長文要約、履歴検索 | — |
| 4 | 署名配布 (notarized DMG / MSIX)、自動アップデート、公開リリース | — |

詳細: [docs/ROADMAP.md](docs/ROADMAP.md) と [docs/PHASE0_POC.md](docs/PHASE0_POC.md)

---

## プライバシー・セキュリティ方針

- **外部接続なし**。アプリ出荷時に network-client エンタイトルメントは無効 (macOS)、Tauri の `http:*` capability 非付与。Little Snitch / `nettop` で何も送信されないことを検証可能。
- **ディスク暗号化**. 全書き起こし・リライトを SQLCipher DB に保管。DB 鍵は OS 鍵ストア (Mac: Keychain + Secure Enclave、Win: DPAPI + TPM) に保管、平文でディスクに出ない。
- **per-session 鍵**. 録音バッファは session ごとの一時鍵を使い終了時に破棄 — 暗号学的消去でファイルシステム削除に依存しない。
- **最小 entitlements**. マイク + アクセシビリティ (テキスト挿入用) のみ。カメラ、位置情報、連絡先、フルディスクアクセスなし。
- **Hardened Runtime + App Sandbox** (macOS)、**AppContainer** (Windows)。
- **テレメトリなし**。将来 opt-in クラッシュ報告を追加する場合もデフォルト OFF、書き起こし本文は絶対に含めない。

---

## ライセンス

- **ソースコード**: [MIT License](LICENSE)
- **LLM モデル** (初回起動時にダウンロード) はそれぞれのライセンスに従います:
  - Whisper: MIT
  - Phi-4-mini: MIT
  - SmolLM3: Apache 2.0
  - Qwen3: Apache 2.0
  - Llama 3.2: Llama 3.2 Community License
  - Gemma 4: Apache 2.0 (Gemma Prohibited Use Policy に同意する必要あり)

ユーザーはダウンロードしたモデルのライセンスに従う責任があります。

---

## コントリビューション

初期段階。Phase 0 完了後に Issues / Discussion を歓迎。PR は OSS コア (ASR、LLM ランタイム、UI、i18n、プラットフォームサポート) 対象。

---

## 謝辞

- [Whisper](https://github.com/openai/whisper) by OpenAI
- [WhisperKit](https://github.com/argmaxinc/WhisperKit) by Argmax
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) by k2-fsa
- [ONNX Runtime GenAI](https://github.com/microsoft/onnxruntime-genai) by Microsoft
- [Tauri](https://tauri.app/)
- [Whispering](https://github.com/epicenter-md/epicenter) — 完全 OSS dictation のアプローチで参考
