"""The scribe's ``llm`` extraction engine (task_36, ADR-026).

Strictly opt-in: the daemon selects this engine only when
``FFS_SCRIBE_ENGINE=llm``. Nothing here runs, and nothing leaves the
machine, under the default ``heuristic`` engine.

Two backends, both plain JSON over HTTP through ``urllib`` (ADR-009,
zero pip dependencies):

- Ollama chat API at ``FFS_SCRIBE_LLM_URL`` (default
  ``http://localhost:11434``): local models, nothing leaves the machine.
- Anthropic Messages API, selected when the URL host is
  ``api.anthropic.com``. The key comes from ``FFS_SCRIBE_ANTHROPIC_KEY``
  or, on macOS, from the login keychain item
  ``ffs-scribe-anthropic`` (``security find-generic-password``).

Anthropic request shape verified 2026-09-20 against the Claude API
reference (platform.claude.com/docs/en/api/messages/create) via the
Context7 documentation index: ``POST /v1/messages`` with headers
``x-api-key``, ``anthropic-version: 2023-06-01``,
``content-type: application/json``; body ``model``, ``max_tokens``,
``messages: [{"role": "user", "content": ...}]``, and ``system`` given
as a list of ``{"type": "text", "text": ...}`` blocks; the reply text
is ``content[0].text``. The documented current Sonnet id is
``claude-sonnet-5``.

Failure posture: any transport, parse, or validation failure falls back
to the ``heuristic`` engine for that submission with a warning, so
ingest never hard-fails because a model or the network did.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, List, Mapping, Optional, Protocol

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)

from ffs_skill import log  # noqa: E402

from engine import EngineResult, Submission, make_proposal  # noqa: E402
from heuristic import HeuristicEngine  # noqa: E402
from prompt import build_prompt  # noqa: E402
from registry import PredicateRegistry  # noqa: E402
from validate import validate_claim  # noqa: E402

DEFAULT_LLM_URL = "http://localhost:11434"
DEFAULT_OLLAMA_MODEL = "llama3.1:8b"
DEFAULT_ANTHROPIC_MODEL = "claude-sonnet-5"
ANTHROPIC_HOST = "api.anthropic.com"
ANTHROPIC_VERSION = "2023-06-01"
ANTHROPIC_MAX_TOKENS = 4096
DEFAULT_TIMEOUT_S = 120.0
KEYCHAIN_SERVICE = "ffs-scribe-anthropic"


class LlmError(Exception):
    """Any failure between the scribe and the model backend."""


# --------------------------------------------------------------------
# Transport
# --------------------------------------------------------------------


class HttpTransport(Protocol):
    def post_json(
        self, url: str, headers: Mapping[str, str], body: Dict[str, Any], timeout: float
    ) -> Dict[str, Any]: ...


class UrllibTransport:
    """``urllib.request`` JSON POST. Raises ``LlmError`` on any failure."""

    def post_json(
        self, url: str, headers: Mapping[str, str], body: Dict[str, Any], timeout: float
    ) -> Dict[str, Any]:
        data = json.dumps(body).encode("utf-8")
        req = urllib.request.Request(url, data=data, method="POST")
        req.add_header("content-type", "application/json")
        for k, v in headers.items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
                raw = resp.read().decode("utf-8", errors="replace")
        except urllib.error.HTTPError as e:
            detail = ""
            try:
                detail = e.read().decode("utf-8", errors="replace")[:300]
            except Exception:  # noqa: BLE001
                pass
            raise LlmError(f"HTTP {e.code} from {url}: {detail}") from e
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise LlmError(f"transport error for {url}: {e}") from e
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError as e:
            raise LlmError(f"non-JSON response from {url}: {raw[:200]!r}") from e
        if not isinstance(parsed, dict):
            raise LlmError(f"unexpected response shape from {url}")
        return parsed


# --------------------------------------------------------------------
# Backends
# --------------------------------------------------------------------


class OllamaBackend:
    name = "ollama"

    def __init__(self, url: str, model: str, transport: HttpTransport, timeout: float = DEFAULT_TIMEOUT_S):
        self.url = url.rstrip("/")
        self.model = model or DEFAULT_OLLAMA_MODEL
        self.transport = transport
        self.timeout = timeout

    def complete(self, system: str, user: str) -> str:
        body = {
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": False,
            "format": "json",
            "options": {"temperature": 0},
        }
        resp = self.transport.post_json(self.url + "/api/chat", {}, body, self.timeout)
        message = resp.get("message")
        if not isinstance(message, dict) or not isinstance(message.get("content"), str):
            raise LlmError("ollama response missing message.content")
        return message["content"]


class AnthropicBackend:
    name = "anthropic"

    def __init__(
        self,
        url: str,
        model: str,
        api_key: Optional[str],
        transport: HttpTransport,
        timeout: float = DEFAULT_TIMEOUT_S,
    ):
        if not api_key:
            raise LlmError(
                "anthropic backend selected but no API key: set FFS_SCRIBE_ANTHROPIC_KEY "
                f"or a macOS keychain item named {KEYCHAIN_SERVICE}"
            )
        self.url = url.rstrip("/")
        self.model = model or DEFAULT_ANTHROPIC_MODEL
        self.api_key = api_key
        self.transport = transport
        self.timeout = timeout

    def complete(self, system: str, user: str) -> str:
        headers = {
            "x-api-key": self.api_key,
            "anthropic-version": ANTHROPIC_VERSION,
            "content-type": "application/json",
        }
        body = {
            "model": self.model,
            "max_tokens": ANTHROPIC_MAX_TOKENS,
            "system": [{"type": "text", "text": system}],
            "messages": [{"role": "user", "content": user}],
        }
        resp = self.transport.post_json(self.url + "/v1/messages", headers, body, self.timeout)
        content = resp.get("content")
        if not isinstance(content, list) or not content:
            raise LlmError("anthropic response missing content blocks")
        first = content[0]
        if not isinstance(first, dict) or not isinstance(first.get("text"), str):
            raise LlmError("anthropic response first block is not text")
        return first["text"]


def _keychain_api_key() -> Optional[str]:
    """Best-effort macOS keychain lookup; never raises."""
    if sys.platform != "darwin":
        return None
    try:
        out = subprocess.run(
            ["security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except Exception:  # noqa: BLE001
        return None
    if out.returncode != 0:
        return None
    key = out.stdout.strip()
    return key or None


def select_backend(
    url: str,
    model: str,
    api_key: Optional[str],
    transport: HttpTransport,
    timeout: float = DEFAULT_TIMEOUT_S,
):
    host = urllib.parse.urlparse(url).hostname or ""
    if host.lower() == ANTHROPIC_HOST:
        return AnthropicBackend(url, model, api_key, transport, timeout)
    return OllamaBackend(url, model, transport, timeout)


# --------------------------------------------------------------------
# Envelope parsing
# --------------------------------------------------------------------


def _strip_fences(text: str) -> str:
    t = text.strip()
    if t.startswith("```"):
        first_nl = t.find("\n")
        t = t[first_nl + 1 :] if first_nl != -1 else ""
        if t.rstrip().endswith("```"):
            t = t.rstrip()[:-3]
    return t.strip()


def _first_balanced_object(text: str) -> Optional[str]:
    start = text.find("{")
    if start == -1:
        return None
    depth = 0
    in_str = False
    esc = False
    for i in range(start, len(text)):
        c = text[i]
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == '"':
                in_str = False
            continue
        if c == '"':
            in_str = True
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    return None


def parse_envelope(text: str) -> Dict[str, Any]:
    """Tolerant envelope extraction: fences and surrounding prose are ignored."""
    candidate = _strip_fences(text)
    try:
        parsed = json.loads(candidate)
    except json.JSONDecodeError:
        obj = _first_balanced_object(candidate)
        if obj is None:
            raise LlmError("no JSON object in model output")
        try:
            parsed = json.loads(obj)
        except json.JSONDecodeError as e:
            raise LlmError(f"model output is not valid JSON: {e}") from e
    if not isinstance(parsed, dict):
        raise LlmError("model output is not a JSON object")
    return parsed


# --------------------------------------------------------------------
# Engine
# --------------------------------------------------------------------


class LlmEngine:
    name = "llm"

    def __init__(self, backend, fallback: Optional[Any] = None):
        self.backend = backend
        self.model = getattr(backend, "model", "")
        self.fallback = fallback or HeuristicEngine()

    @classmethod
    def from_env(cls, env: Optional[Mapping[str, str]] = None, transport: Optional[HttpTransport] = None) -> "LlmEngine":
        env = env if env is not None else os.environ
        url = env.get("FFS_SCRIBE_LLM_URL") or DEFAULT_LLM_URL
        model = env.get("FFS_SCRIBE_LLM_MODEL") or ""
        api_key: Optional[str] = env.get("FFS_SCRIBE_ANTHROPIC_KEY") or None
        host = urllib.parse.urlparse(url).hostname or ""
        if api_key is None and host.lower() == ANTHROPIC_HOST:
            api_key = _keychain_api_key()
        backend = select_backend(url, model, api_key, transport or UrllibTransport())
        return cls(backend)

    def _fallback(self, submission: Submission, registry: PredicateRegistry, reason: str) -> EngineResult:
        log("warn", f"llm engine failed ({reason}); heuristic fallback used")
        result = self.fallback.extract(submission, registry)
        warnings = [f"llm engine failed ({reason}); heuristic fallback used"] + list(result.warnings)
        return EngineResult(proposals=list(result.proposals), warnings=warnings)

    def extract(self, submission: Submission, registry: PredicateRegistry) -> EngineResult:
        try:
            system, user = build_prompt(registry, submission)
            started = time.monotonic()
            text = self.backend.complete(system, user)
            elapsed_ms = int((time.monotonic() - started) * 1000)
            log("info", f"llm engine: backend={self.backend.name} model={self.model} latency_ms={elapsed_ms}")
            envelope = parse_envelope(text)
        except Exception as e:  # noqa: BLE001
            return self._fallback(submission, registry, f"{type(e).__name__}: {e}")

        raw_proposals = envelope.get("proposals")
        if not isinstance(raw_proposals, list):
            return self._fallback(submission, registry, "envelope has no proposals array")

        summary = envelope.get("summary")
        warnings: List[str] = []
        proposals: List[Dict[str, Any]] = []
        for idx, item in enumerate(raw_proposals):
            if not isinstance(item, dict):
                warnings.append(f"proposal #{idx} is not an object; dropped")
                continue
            predicate = item.get("predicate")
            claim = item.get("claim")
            if not isinstance(predicate, str) or not registry.has(predicate):
                warnings.append(f"proposal #{idx}: unregistered predicate {predicate!r}; dropped")
                continue
            err = validate_claim(claim, registry.schema(predicate))
            if err is not None:
                warnings.append(f"proposal #{idx} ({predicate}): {err}; dropped")
                continue
            rationale = str(item.get("rationale") or "extracted by model")
            if summary and not proposals and isinstance(summary, str):
                rationale = f"{rationale} (llm; summary: {summary.strip()})"
            else:
                rationale = f"{rationale} (llm)"
            proposals.append(
                make_proposal(predicate, claim, submission, rationale, self.name, self.model)
            )

        if not proposals:
            result = self._fallback(submission, registry, "zero schema-valid proposals")
            result.warnings.extend(warnings)
            return result
        return EngineResult(proposals=proposals, warnings=warnings)
