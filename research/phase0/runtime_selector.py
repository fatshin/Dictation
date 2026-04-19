"""Execution-provider selection for onnxruntime / onnxruntime-genai."""

from __future__ import annotations

import argparse
import platform
import sys
from typing import Callable


def detect_platform() -> str:
    """Return a short platform tag used by the bench scripts."""
    if sys.platform == "darwin":
        return "macos-arm64" if platform.machine() in {"arm64", "aarch64"} else "macos-x64"
    if sys.platform == "win32":
        machine = platform.machine().lower()
        if machine in {"arm64", "aarch64"}:
            return "windows-arm64"
        return "windows-x64"
    return "cpu"


def available_providers() -> list[str]:
    """Wrap onnxruntime.get_available_providers() so callers don't import ORT directly."""
    import onnxruntime as ort

    return list(ort.get_available_providers())


def _has(provider: str) -> bool:
    return provider in available_providers()


def _genai_has(check: Callable[[], bool]) -> bool:
    # genai exposes is_qnn_available / is_openvino_available / is_dml_available;
    # fall back to False if the runtime doesn't carry them.
    try:
        return bool(check())
    except Exception:
        return False


def select_execution_providers() -> list[str]:
    """Pick the ordered EP list for the current OS, falling back to CPU.

    macOS: CoreML -> CPU
    Windows: QNN -> OpenVINO -> DirectML -> CPU (first one that is present)
    Other:   CPU only
    """
    tag = detect_platform()

    if tag.startswith("macos"):
        chain: list[str] = []
        if _has("CoreMLExecutionProvider"):
            chain.append("CoreMLExecutionProvider")
        chain.append("CPUExecutionProvider")
        return chain

    if tag.startswith("windows"):
        import onnxruntime_genai as og

        for ep, check in (
            ("QNNExecutionProvider", og.is_qnn_available),
            ("OpenVINOExecutionProvider", og.is_openvino_available),
            ("DmlExecutionProvider", og.is_dml_available),
        ):
            if _has(ep) and _genai_has(check):
                return [ep, "CPUExecutionProvider"]
        if _has("DmlExecutionProvider"):
            return ["DmlExecutionProvider", "CPUExecutionProvider"]
        return ["CPUExecutionProvider"]

    return ["CPUExecutionProvider"]


def _cli() -> None:
    parser = argparse.ArgumentParser(description="Runtime selector diagnostics.")
    parser.add_argument("command", choices=["platform", "providers", "selected"])
    args = parser.parse_args()

    if args.command == "platform":
        print(detect_platform())
    elif args.command == "providers":
        for p in available_providers():
            print(p)
    else:
        for p in select_execution_providers():
            print(p)


if __name__ == "__main__":
    _cli()
