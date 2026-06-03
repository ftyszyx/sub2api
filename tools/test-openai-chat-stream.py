#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

# Defaults. Command-line args can override them.
BASE_URL = "https://gpt-proxy-usa-pub.singularity-ai.com/gpt-proxy/api"
API_KEY = os.getenv("OPENAI_API_KEY", "gpt-254f442bbfd7a52afaffce414106")
MODEL = "gpt-5.5"
PROMPT = "hi"


def codex_responses_url(base_url: str) -> str:
    base = base_url.strip().rstrip("/")
    if base.endswith("/responses"):
        return base
    return f"{base}/responses"


def build_payload(model: str, prompt: str, stream: bool) -> dict:
    return {
        "model": model,
        "instructions": "You are Codex. Reply briefly.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": prompt}],
            }
        ],
        "tool_choice": "auto",
        "parallel_tool_calls": True,
        "reasoning": {"effort": "high"},
        "store": False,
        "stream": stream,
        "prompt_cache_key": "sub2api-script-test",
        "text": {"verbosity": "low"},
        "client_metadata": {"x-codex-installation-id": "sub2api-script-test"},
    }


def decode_body(body: bytes) -> str:
    return body.decode("utf-8", errors="replace")


def diagnose(body_text: str, stream: bool) -> str:
    stripped = body_text.lstrip()
    if not stream:
        return "OK: non-stream response looks like JSON." if stripped.startswith("{") else "WARN: non-stream response is not JSON."
    if "response.completed" in body_text or "response.done" in body_text:
        return "OK: stream contains response.completed/response.done."
    if stripped.startswith("{"):
        return "WARN: got plain JSON, not SSE. Upstream may be using the wrong route."
    if stripped.startswith("<"):
        return "WARN: got HTML. BaseUrl may point to a web page or proxy error."
    if not stripped:
        return "WARN: empty body."
    return "WARN: stream has no terminal response.completed/response.done event."


def request_once(endpoint: str, api_key: str, payload: dict) -> tuple[int, str, str, bytes, float]:
    payload_bytes = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "Accept": "text/event-stream",
        "OpenAI-Beta": "responses=experimental",
        "Originator": "codex_vscode",
        "User-Agent": "codex_vscode/0.136.0 (Windows 10.0.19044; x86_64) dumb (sub2api_script; 0.136.0)",
        "x-codex-beta-features": "terminal_resize_reflow",
        "x-client-request-id": "sub2api-script-test",
        "session-id": "sub2api-script-test",
        "thread-id": "sub2api-script-test",
    }
    req = urllib.request.Request(endpoint, data=payload_bytes, headers=headers, method="POST")

    started = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            body = resp.read()
            elapsed = time.perf_counter() - started
            return resp.status, resp.reason, str(resp.headers), body, elapsed
    except urllib.error.HTTPError as exc:
        body = exc.read()
        elapsed = time.perf_counter() - started
        return exc.code, exc.reason, str(exc.headers), body, elapsed
    except urllib.error.URLError as exc:
        elapsed = time.perf_counter() - started
        return 0, f"network error: {exc.reason}", "", b"", elapsed


def main() -> int:
    parser = argparse.ArgumentParser(description="Test a Codex-style /responses SSE endpoint.")
    parser.add_argument("--base-url", default=BASE_URL)
    parser.add_argument("--api-key", default=API_KEY)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--prompt", default=PROMPT)
    parser.add_argument("--non-stream", action="store_true")
    args = parser.parse_args()

    if not args.api_key:
        print("ERROR: set API_KEY in this file, pass --api-key, or set OPENAI_API_KEY.", file=sys.stderr)
        return 2

    endpoint = codex_responses_url(args.base_url)
    stream = not args.non_stream
    payload = build_payload(args.model, args.prompt, stream)

    status, reason, headers_text, body, elapsed = request_once(endpoint, args.api_key, payload)

    body_text = decode_body(body)

    print(f"Endpoint: {endpoint}")
    print(f"Model:    {args.model}")
    print(f"Stream:   {stream}")
    print(f"HTTP:     {status} {reason}")
    print(f"Time:     {elapsed:.3f}s")
    print()

    print("=== Response headers ===")
    print(headers_text.rstrip())
    print()

    preview = body_text[:4000]
    print("=== Body preview ===")
    print(preview)
    if len(body_text) > len(preview):
        print("\n... body truncated")
    print("\n=== Diagnosis ===")
    print(diagnose(body_text, stream))

    return 0 if 200 <= status < 300 else 1


if __name__ == "__main__":
    raise SystemExit(main())
