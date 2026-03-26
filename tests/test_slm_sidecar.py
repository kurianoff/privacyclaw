"""
Integration tests for packaging/privacyclaw-slm-sidecar.

Uses FastAPI's TestClient / httpx.AsyncClient with ASGITransport for in-process
testing. LLM calls are mocked via monkeypatching call_llm_for_pii.
"""

import importlib.util
import os
import sys
from pathlib import Path
from typing import AsyncGenerator
from unittest.mock import AsyncMock, patch

import httpx
import pytest
import pytest_asyncio
from fastapi.testclient import TestClient

# ── Load the sidecar module from its path (not a package) ────────────────────

_SIDECAR_PATH = (
    Path(__file__).parent.parent / "packaging" / "privacyclaw-slm-sidecar"
)


def _load_sidecar():
    """Dynamically import the sidecar script as a module (no .py extension)."""
    import importlib.machinery
    loader = importlib.machinery.SourceFileLoader(
        "privacyclaw_slm_sidecar", str(_SIDECAR_PATH)
    )
    spec = importlib.util.spec_from_loader(
        "privacyclaw_slm_sidecar", loader
    )
    module = importlib.util.module_from_spec(spec)
    # Register in sys.modules so internal references resolve correctly.
    sys.modules["privacyclaw_slm_sidecar"] = module
    loader.exec_module(module)
    return module


_sidecar = _load_sidecar()
app = _sidecar.app


# ── Fixtures ──────────────────────────────────────────────────────────────────


@pytest.fixture
def client():
    """Sync TestClient that runs startup/shutdown lifecycle."""
    with TestClient(app) as c:
        yield c


@pytest.fixture(autouse=True)
def reset_llama_ready():
    """Reset _llama_ready to True before each test (default: pass-through mode)."""
    _sidecar._llama_ready = True
    yield
    _sidecar._llama_ready = True


# ── Tests ─────────────────────────────────────────────────────────────────────


def test_health_returns_ok(client):
    """GET /health returns 200 when _llama_ready is True."""
    _sidecar._llama_ready = True
    resp = client.get("/health")
    assert resp.status_code == 200
    assert resp.json() == {"status": "ok"}


def test_health_returns_503_when_not_ready(client):
    """GET /health returns 503 when _llama_ready is False."""
    _sidecar._llama_ready = False
    resp = client.get("/health")
    assert resp.status_code == 503
    assert resp.json() == {"status": "starting"}


def test_replace_success(client):
    """
    POST /replace: mock LLM returns ["Anne Nicole"];
    response has one replacement with pii_type="person_name" and non-empty display_value.
    """
    with patch.object(
        _sidecar,
        "call_llm_for_pii",
        new=AsyncMock(return_value=["Anne Nicole"]),
    ):
        resp = client.post("/replace", json={"text": "My name is Anne Nicole."})
    assert resp.status_code == 200
    data = resp.json()
    assert len(data["replacements"]) == 1
    r = data["replacements"][0]
    assert r["pii_type"] == "person_name"
    assert r["display_value"]  # non-empty
    assert r["original"] == "Anne Nicole"
    assert r["start"] == 11
    assert r["end"] == 22


def test_replace_text_too_large(client):
    """POST /replace with text > 32,768 chars returns HTTP 400."""
    large_text = "A" * 32_769
    resp = client.post("/replace", json={"text": large_text})
    assert resp.status_code == 400
    assert resp.json()["detail"] == "text too large"


def test_replace_llm_timeout(client):
    """POST /replace: mock LLM returns [] (timeout scenario) → HTTP 200 with empty list."""
    with patch.object(
        _sidecar,
        "call_llm_for_pii",
        new=AsyncMock(return_value=[]),
    ):
        resp = client.post("/replace", json={"text": "Some text with Anne Nicole."})
    assert resp.status_code == 200
    assert resp.json()["replacements"] == []


def test_replace_overlap_deduplication(client):
    """
    Mock LLM returns ["Anne Nicole", "Anne"]; text is "Anne Nicole".
    resolve_replacements must return only one replacement (longer match wins).
    """
    with patch.object(
        _sidecar,
        "call_llm_for_pii",
        new=AsyncMock(return_value=["Anne Nicole", "Anne"]),
    ):
        resp = client.post("/replace", json={"text": "Anne Nicole"})
    assert resp.status_code == 200
    data = resp.json()
    # Only "Anne Nicole" matches — "Anne" is covered by the longer span.
    assert len(data["replacements"]) == 1
    assert data["replacements"][0]["original"] == "Anne Nicole"


def test_replace_overlap_second_occurrence(client):
    """
    Text: "Anne Nicole called Anne about 333-444-5555"
    LLM: ["Anne Nicole", "Anne", "333-444-5555"]
    Expected: 3 replacements:
      - "Anne Nicole" at offset 0
      - "Anne" at offset 20 (second occurrence, not covered)
      - "333-444-5555" at its offset
    """
    text = "Anne Nicole called Anne about 333-444-5555"
    with patch.object(
        _sidecar,
        "call_llm_for_pii",
        new=AsyncMock(return_value=["Anne Nicole", "Anne", "333-444-5555"]),
    ):
        resp = client.post("/replace", json={"text": text})
    assert resp.status_code == 200
    data = resp.json()
    originals = [r["original"] for r in data["replacements"]]
    assert "Anne Nicole" in originals
    assert "Anne" in originals
    assert "333-444-5555" in originals
    assert len(data["replacements"]) == 3
    # Sorted by start ascending.
    starts = [r["start"] for r in data["replacements"]]
    assert starts == sorted(starts)


def test_chat_completions_nonstreaming(client):
    """
    POST /v1/chat/completions without stream:true → response body from mock
    llama-server returned unchanged.
    """
    mock_response_body = b'{"choices":[{"message":{"content":"hello"}}]}'

    async def mock_post(*args, **kwargs):
        return httpx.Response(
            200,
            content=mock_response_body,
            headers={"content-type": "application/json"},
        )

    with patch.object(httpx.AsyncClient, "post", new=AsyncMock(side_effect=mock_post)):
        resp = client.post(
            "/v1/chat/completions",
            json={"model": "local", "messages": [{"role": "user", "content": "hi"}]},
        )
    assert resp.status_code == 200
    assert resp.content == mock_response_body


def test_chat_completions_streaming(client):
    """
    POST /v1/chat/completions with stream:true → StreamingResponse with
    text/event-stream media type and streamed chunks.
    """
    chunks = [b"data: chunk1\n\n", b"data: chunk2\n\n"]

    # We need to mock the httpx streaming path.
    # TestClient collects the full streaming response body.
    class _MockStreamCtx:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_):
            pass

        async def aiter_bytes(self):
            for chunk in chunks:
                yield chunk

    def mock_stream(*args, **kwargs):
        return _MockStreamCtx()

    with patch.object(httpx.AsyncClient, "stream", side_effect=mock_stream):
        resp = client.post(
            "/v1/chat/completions",
            json={
                "model": "local",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": True,
            },
        )
    assert resp.status_code == 200
    assert "text/event-stream" in resp.headers["content-type"]
    assert b"chunk1" in resp.content
    assert b"chunk2" in resp.content


# ── Unit tests for helper functions ──────────────────────────────────────────


def test_classify_pii_email():
    assert _sidecar.classify_pii_type("test@example.com") == "email"


def test_classify_pii_ssn():
    assert _sidecar.classify_pii_type("333-22-4444") == "ssn"


def test_classify_pii_phone():
    assert _sidecar.classify_pii_type("333-444-5555") == "phone"


def test_classify_pii_person_name():
    assert _sidecar.classify_pii_type("Anne Nicole") == "person_name"


def test_generate_synthetic_person_name_deterministic():
    result1 = _sidecar.generate_synthetic("Anne Nicole", "person_name")
    result2 = _sidecar.generate_synthetic("Anne Nicole", "person_name")
    assert result1 == result2
    assert result1 in _sidecar.NAMES


def test_generate_synthetic_email():
    result = _sidecar.generate_synthetic("user@corp.com", "email")
    import re
    assert re.match(r"redacted[0-9a-f]{4}@example\.com", result)


def test_generate_synthetic_fallback():
    result = _sidecar.generate_synthetic("unknown-pii-value", "other_pii")
    assert result == "[REDACTED]"


def test_names_pool_length():
    assert len(_sidecar.NAMES) == 50


def test_names_pool_format():
    import re
    pattern = re.compile(r"^[A-Za-zÀ-ÖØ-öø-ÿ]+ [A-Za-zÀ-ÖØ-öø-ÿ]+$")
    for name in _sidecar.NAMES:
        assert pattern.match(name), f"Name does not match pattern: {name!r}"


def test_markdown_fence_stripping():
    raw = '```json\n["Alice"]\n```'
    cleaned = _sidecar._strip_markdown_fences(raw)
    assert cleaned == '["Alice"]'


def test_markdown_fence_stripping_no_fence():
    raw = '["Alice"]'
    assert _sidecar._strip_markdown_fences(raw) == '["Alice"]'


def test_replace_response_default_construction():
    r = _sidecar.ReplaceResponse()
    assert r.modified_text == ""
    assert r.replacements == []
    assert r.model_dump() == {"modified_text": "", "replacements": []}
