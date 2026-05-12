"""Unit tests for pure helper functions (Day 2.5 addition).

Scope: only imports that do not pull ORT/GenAI/Anthropic, so this runs in a
minimal Python env. Heavier I/O paths are covered by the bench runs themselves.
"""

from __future__ import annotations

import sys
import json
import types
from pathlib import Path

PHASE0_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PHASE0_ROOT))

import pytest  # noqa: E402

from aggregate import _percentile, summarize  # noqa: E402
from bench_asr import _infer_lang  # noqa: E402
from bench_llm import _extract_section, _read_model_repo_revision, _render_prompt, _task_type_for  # noqa: E402
from models import _manifest_entry_has_hashes, _manifest_files  # noqa: E402


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


class TestAggregateJudgeCoverage:
    def test_unjudged_model_is_blocked_not_zero_quality(self) -> None:
        rows = [{
            "model_alias": "m",
            "ttft_ms": 1000.0,
            "tokens_per_sec": 20.0,
            "peak_ram_mb": 1000.0,
            "input_hash": "in",
            "output_hash": "out",
        }]

        summary = summarize(rows, {})[0]

        assert summary.judged_runs == 0
        assert summary.quality_avg is None
        assert summary.pass_quality is False
        assert summary.verdict == "BLOCKED"

    def test_fully_judged_model_can_pass_quality(self) -> None:
        rows = [{
            "model_alias": "m",
            "ttft_ms": 1000.0,
            "tokens_per_sec": 20.0,
            "peak_ram_mb": 1000.0,
            "input_hash": "in",
            "output_hash": "out",
        }]
        judge = {
            ("m", "in", "out"): {
                "keigo": 8.0,
                "filler": 8.0,
                "semantic": 8.0,
                "structure": 8.0,
            }
        }

        summary = summarize(rows, judge)[0]

        assert summary.judged_runs == 1
        assert summary.quality_avg == 8.0
        assert summary.pass_quality is True
        assert summary.verdict == "PASS"


class TestInferLang:
    def test_japanese_only(self) -> None:
        assert _infer_lang("これはテスト") == "ja"

    def test_english_only(self) -> None:
        assert _infer_lang("This is a test") == "en"

    def test_mixed_prefers_japanese(self) -> None:
        """Whisper hint must be 'ja' for JP-dominant-mixed input, else whisper
        translates to English rather than transcribing (Day-2.5 finding)."""
        assert _infer_lang("これは API のテスト") == "ja"

    def test_empty_is_none(self) -> None:
        assert _infer_lang("12345") is None


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


class TestManifestCompatibility:
    def test_manifest_files_legacy_format(self) -> None:
        entry = {"a.bin": "abc", "b.bin": "def"}
        assert _manifest_files(entry) == entry

    def test_manifest_files_metadata_format(self) -> None:
        entry = {"repo_id": "x/y", "revision": "rev", "files": {"a.bin": "abc"}}
        assert _manifest_files(entry) == {"a.bin": "abc"}

    def test_manifest_files_metadata_without_files_returns_empty(self) -> None:
        entry = {"repo_id": "x/y", "revision": "rev"}
        assert _manifest_files(entry) == {}

    def test_manifest_entry_has_hashes_false_when_metadata_missing_files(self) -> None:
        entry = {"repo_id": "x/y", "revision": "rev"}
        assert _manifest_entry_has_hashes(entry) is False

    def test_manifest_entry_has_hashes_true_for_legacy_file_map(self) -> None:
        entry = {"a.bin": "abc"}
        assert _manifest_entry_has_hashes(entry) is True

    def test_read_model_repo_revision_uses_manifest_metadata(self, tmp_path: Path) -> None:
        import bench_llm as mod

        results = tmp_path / "results"
        results.mkdir(parents=True, exist_ok=True)
        model_dir = tmp_path / "downloads" / "my-model"
        model_dir.mkdir(parents=True, exist_ok=True)
        manifest_path = results / "model_manifest.json"
        manifest_path.write_text(json.dumps({
            "my-model": {
                "repo_id": "org/my-model",
                "revision": "abc123",
                "files": {"weights.bin": "deadbeef"},
            }
        }))

        old = mod.RESULTS_DIR
        mod.RESULTS_DIR = results
        try:
            repo, revision = _read_model_repo_revision("my-model", model_dir)
        finally:
            mod.RESULTS_DIR = old

        assert repo == "org/my-model"
        assert revision == "abc123"

    def test_read_model_repo_revision_prefers_manifest_over_registry(self, tmp_path: Path) -> None:
        import bench_llm as mod
        import models as models_mod

        results = tmp_path / "results"
        results.mkdir(parents=True, exist_ok=True)
        model_dir = tmp_path / "downloads" / "qwen2_5-0_5b-int4"
        model_dir.mkdir(parents=True, exist_ok=True)
        manifest_path = results / "model_manifest.json"
        manifest_path.write_text(json.dumps({
            "qwen2_5-0_5b-int4": {
                "repo_id": "custom/override-repo",
                "revision": "rev999",
                "files": {"weights.bin": "deadbeef"},
            }
        }))

        old_results = mod.RESULTS_DIR
        old_models = models_mod.ALL_MODELS
        mod.RESULTS_DIR = results
        models_mod.ALL_MODELS = {"qwen2_5-0_5b-int4": "stale/registry-repo"}
        try:
            repo, revision = _read_model_repo_revision("qwen2_5-0_5b-int4", model_dir)
        finally:
            mod.RESULTS_DIR = old_results
            models_mod.ALL_MODELS = old_models

        assert repo == "custom/override-repo"
        assert revision == "rev999"


class TestDownloadManifestMetadata:
    def test_cmd_download_continues_when_existence_probe_raises_systemexit(
        self,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        import models as models_mod

        models_dir = tmp_path / "downloads"
        manifest_path = tmp_path / "results" / "model_manifest.json"
        monkeypatch.setattr(models_mod, "MODELS_DIR", models_dir)
        monkeypatch.setattr(models_mod, "MANIFEST_PATH", manifest_path)
        monkeypatch.setattr(models_mod, "TIER_1", {"m1": "org/repo-m1"})

        def _check_existence(_: str) -> dict:
            raise SystemExit("probe failed")

        def _download(_: str, dest: Path, revision: str | None = None) -> Path:
            assert revision is None
            dest.mkdir(parents=True, exist_ok=True)
            (dest / "weights.bin").write_text("x")
            return dest

        monkeypatch.setattr(models_mod, "check_existence", _check_existence)
        monkeypatch.setattr(models_mod, "download", _download)
        monkeypatch.setattr(models_mod, "build_manifest", lambda _p: {"weights.bin": "deadbeef"})

        models_mod._cmd_download(types.SimpleNamespace(tier="1"))

        manifest = json.loads(manifest_path.read_text())
        assert manifest["m1"]["repo_id"] == "org/repo-m1"
        assert manifest["m1"]["revision"] == ""
        assert manifest["m1"]["files"]["weights.bin"] == "deadbeef"

    def test_cmd_download_passes_revision_from_probe(
        self,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        import models as models_mod

        models_dir = tmp_path / "downloads"
        manifest_path = tmp_path / "results" / "model_manifest.json"
        monkeypatch.setattr(models_mod, "MODELS_DIR", models_dir)
        monkeypatch.setattr(models_mod, "MANIFEST_PATH", manifest_path)
        monkeypatch.setattr(models_mod, "TIER_2", {"m2": "org/repo-m2"})

        captured_revision: list[str | None] = []

        def _check_existence(_: str) -> dict:
            return {"revision": "abc123"}

        def _download(_: str, dest: Path, revision: str | None = None) -> Path:
            captured_revision.append(revision)
            dest.mkdir(parents=True, exist_ok=True)
            (dest / "weights.bin").write_text("x")
            return dest

        monkeypatch.setattr(models_mod, "check_existence", _check_existence)
        monkeypatch.setattr(models_mod, "download", _download)
        monkeypatch.setattr(models_mod, "build_manifest", lambda _p: {"weights.bin": "deadbeef"})

        models_mod._cmd_download(types.SimpleNamespace(tier="2"))

        assert captured_revision == ["abc123"]
        manifest = json.loads(manifest_path.read_text())
        assert manifest["m2"]["revision"] == "abc123"
