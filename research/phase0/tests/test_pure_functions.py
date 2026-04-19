"""Unit tests for pure helper functions (Day 2.5 addition).

Scope: only imports that do not pull ORT/GenAI/Anthropic, so this runs in a
minimal Python env. Heavier I/O paths are covered by the bench runs themselves.
"""

from __future__ import annotations

import sys
from pathlib import Path

PHASE0_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PHASE0_ROOT))

import pytest  # noqa: E402

from aggregate import _percentile  # noqa: E402
from bench_asr import _infer_lang  # noqa: E402
from bench_llm import _extract_section, _render_prompt, _task_type_for  # noqa: E402


class TestExtractSection:
    def test_pulls_only_named_section(self) -> None:
        text = "## INPUT\nhello\n## EXPECTED\nbye\n"
        assert _extract_section(text, "INPUT") == "hello"
        assert _extract_section(text, "EXPECTED") == "bye"

    def test_last_section_captures_rest(self) -> None:
        text = "## INPUT\nonly one section"
        assert _extract_section(text, "INPUT") == "only one section"

    def test_missing_section_raises(self) -> None:
        with pytest.raises(SystemExit):
            _extract_section("## OTHER\nx", "INPUT")


class TestPercentile:
    def test_median(self) -> None:
        assert _percentile([1, 2, 3, 4, 5], 0.5) == 3

    def test_p95(self) -> None:
        # 100 items, p95 == 95th element (1-indexed 96th) under linear interp
        data = list(range(100))
        assert _percentile(data, 0.95) == pytest.approx(94.05, rel=0.01)

    def test_empty_is_inf(self) -> None:
        assert _percentile([], 0.5) == float("inf")


class TestInferLang:
    def test_japanese_only(self) -> None:
        assert _infer_lang("これはテスト") == "ja"

    def test_english_only(self) -> None:
        assert _infer_lang("This is a test") == "en"

    def test_mixed_is_none(self) -> None:
        assert _infer_lang("これは test です") is None


class TestTaskType:
    @pytest.mark.parametrize("wid,expected", [
        ("ja_keigo_01", "ja_keigo"),
        ("jp_en_mix_03", "jp_en_mix"),
        ("en_business_05", "en_business"),
        ("summary_long_02", "summary"),
    ])
    def test_derivation(self, wid: str, expected: str) -> None:
        assert _task_type_for(wid) == expected

    def test_unknown_raises(self) -> None:
        with pytest.raises(SystemExit):
            _task_type_for("nonsense_99")


class TestPromptTemplate:
    def test_japanese_prompt_requires_japanese_output(self) -> None:
        """Regression guard for the Day-2 bug: Phi emitted English for JP input."""
        p = _render_prompt("入力テスト", "ja_keigo_01")
        assert "日本語" in p
        assert "敬体" in p or "です・ます" in p
        assert "入力テスト" in p

    def test_en_prompt_is_english_only(self) -> None:
        p = _render_prompt("raw text", "en_business_01")
        assert "English only" in p
        assert "raw text" in p

    def test_mix_prompt_preserves_main_language(self) -> None:
        p = _render_prompt("あ API b", "jp_en_mix_02")
        assert "主言語" in p or "技術用語" in p
