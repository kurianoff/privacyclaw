"""
conftest.py — shared pytest configuration for privacyclaw-slm-sidecar tests.

Clears proxy-related environment variables before every test session so that
httpx.AsyncClient does not attempt to initialise a SOCKS transport (which
requires the optional socksio package).  The ALL_PROXY / HTTPS_PROXY /
HTTP_PROXY variables are restored after the session ends.
"""

import os
import pytest

_PROXY_VARS = ("ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY",
               "all_proxy", "https_proxy", "http_proxy", "no_proxy")


@pytest.fixture(scope="session", autouse=True)
def clear_proxy_env():
    """Remove proxy env-vars for the entire test session."""
    saved = {k: os.environ.pop(k) for k in _PROXY_VARS if k in os.environ}
    yield
    os.environ.update(saved)
