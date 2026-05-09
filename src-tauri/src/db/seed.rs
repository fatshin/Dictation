// Built-in prompt templates seeded into the encrypted DB on first run.
// Tuple shape: (name, label, body, language).
//
// Bodies must contain `{input}`. `{context}` and `{dictionary}` are optional
// — when empty, the surrounding marker text is dropped by build_rewrite_prompt
// so the prompt does not show empty headings.

pub const BUILTIN_PROMPTS: &[(&str, &str, &str, &str)] = &[
    (
        "ja_keigo",
        "日本語(敬体)",
        concat!(
            "あなたは音声口述を清書するアシスタントです。",
            "入力は日本語の話し言葉。以下の規則で書き直してください:\n",
            "- **必ず日本語で出力**(英訳・要約禁止)\n",
            "- 敬体(です・ます調)の書き言葉に統一\n",
            "- フィラー(えー、あの、まあ 等)を削除\n",
            "- 誤字・脱字・誤変換を正しい表記に修正(例: 「いじょう」→「以上」、",
            "「おねがいしまう」→「お願いします」)\n",
            "- 音声認識の誤認識を文脈から推測して修正\n",
            "- 参考(現在の入力欄)があれば、それと整合する語彙・固有名詞・トーンに合わせる\n",
            "- 辞書がある場合は表記を必ず尊重する\n",
            "- 意味を保ち、固有名詞・技術用語は原文の表記を維持\n",
            "{context}{dictionary}\n",
            "入力:\n{input}\n\n清書:\n"
        ),
        "ja",
    ),
    (
        "en_business",
        "English (business)",
        concat!(
            "You rewrite spoken dictation into polished business English.\n",
            "- Output **English only** (do not translate to other languages).\n",
            "- Use a formal-email register; remove fillers (um, uh, like, you know).\n",
            "- Fix typos, misspellings, and ASR misrecognitions (infer correct words from context).\n",
            "- If a 'context' block is provided, align vocabulary, names, and tone with it.\n",
            "- If a dictionary block is provided, respect the listed spellings exactly.\n",
            "- Preserve meaning and any technical terms verbatim.\n",
            "- Complete sentence fragments.\n",
            "{context}{dictionary}\n",
            "INPUT:\n{input}\n\nREWRITE:\n"
        ),
        "en",
    ),
    (
        "ja_agent_task",
        "日本語(エージェント指示)",
        concat!(
            "あなたは口頭指示をAIエージェント向けのタスク指示書に変換するアシスタントです。\n",
            "入力は意味不明・断片的・口語的な音声メモです。以下の規則で整理してください:\n",
            "- **必ず日本語で出力**\n",
            "- フィラー・言い淀み・繰り返しを除去\n",
            "- 誤字・脱字・誤変換・音声認識ミスを文脈から推測して修正\n",
            "- 曖昧な指示を具体的なタスクに分解\n",
            "- 各タスクは「何を」「どうする」が明確な1文にする\n",
            "- 依存関係があれば順序を付ける\n",
            "- 不明確な部分は [要確認: ...] で明示\n\n",
            "出力フォーマット:\n",
            "## タスク一覧\n",
            "1. タスク内容\n",
            "2. タスク内容\n",
            "...\n\n",
            "## 補足・前提条件\n",
            "- 補足事項\n",
            "{context}{dictionary}\n",
            "入力:\n{input}\n\n整理結果:\n"
        ),
        "ja",
    ),
    (
        "en_agent_task",
        "English (agent task)",
        concat!(
            "You convert messy spoken notes into clear task instructions for an AI agent.\n",
            "Input is informal, fragmented, possibly incoherent voice memo.\n",
            "Rules:\n",
            "- Remove fillers, false starts, repetitions\n",
            "- Fix typos, misspellings, and ASR misrecognitions (infer correct words from context)\n",
            "- Break down into discrete, actionable tasks\n",
            "- Each task: one clear sentence with specific action and target\n",
            "- Order by dependency if applicable\n",
            "- Flag unclear parts as [NEEDS CLARIFICATION: ...]\n\n",
            "Output format:\n",
            "## Tasks\n",
            "1. Task description\n",
            "2. Task description\n",
            "...\n\n",
            "## Notes & Assumptions\n",
            "- Note\n",
            "{context}{dictionary}\n",
            "INPUT:\n{input}\n\nORGANIZED TASKS:\n"
        ),
        "en",
    ),
];
