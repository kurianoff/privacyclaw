"""
Tests for packaging/homebrew/privacyclaw.rb formula correctness.

Covers the five risk areas flagged in the development handoff:
  1. Formula Ruby syntax validity (structural checks without running brew)
  2. --version flag exits before dependency guard (sidecar script)
  3. test-block assertion uses correct string and is inside test do...end
  4. pydantic-core on_macos/on_arm + on_macos/on_intel block structure
  5. inreplace regex correctness (\\A anchor, opt_libexec reference)

All tests parse the formula file as text; no brew installation required.
"""

import re
import subprocess
import sys
from pathlib import Path

import pytest

# ── Locate artifacts ──────────────────────────────────────────────────────────

_REPO_ROOT = Path(__file__).parent.parent
_FORMULA = _REPO_ROOT / "packaging" / "homebrew" / "privacyclaw.rb"
_SIDECAR = _REPO_ROOT / "packaging" / "privacyclaw-slm-sidecar"


@pytest.fixture(scope="module")
def formula_text():
    """Read the formula file once per test module."""
    assert _FORMULA.exists(), f"Formula not found at {_FORMULA}"
    return _FORMULA.read_text(encoding="utf-8")


@pytest.fixture(scope="module")
def sidecar_text():
    """Read the sidecar script once per test module."""
    assert _SIDECAR.exists(), f"Sidecar not found at {_SIDECAR}"
    return _SIDECAR.read_text(encoding="utf-8")


# ── 1. Formula structural validity ────────────────────────────────────────────


def test_formula_declares_class(formula_text):
    """Formula must declare the Homebrew Formula class."""
    assert "class Privacyclaw < Formula" in formula_text


def test_formula_has_desc(formula_text):
    """Formula must have a desc field."""
    assert re.search(r'desc\s+"', formula_text)


def test_formula_has_homepage(formula_text):
    """Formula must have a homepage field."""
    assert re.search(r'homepage\s+"', formula_text)


def test_formula_has_version(formula_text):
    """Formula must have a version field."""
    assert re.search(r'version\s+"', formula_text)


def test_formula_has_depends_on_llama_cpp(formula_text):
    """Formula must depend on llama.cpp."""
    assert 'depends_on "llama.cpp"' in formula_text


def test_formula_has_depends_on_python311(formula_text):
    """Formula must depend on python@3.11 (Task 2.1)."""
    assert 'depends_on "python@3.11"' in formula_text


def test_python311_depends_on_after_llama_cpp(formula_text):
    """depends_on python@3.11 must appear immediately after depends_on llama.cpp."""
    lines = formula_text.splitlines()
    llama_idx = next(
        (i for i, line in enumerate(lines) if 'depends_on "llama.cpp"' in line), None
    )
    assert llama_idx is not None, "depends_on llama.cpp not found"
    python_idx = next(
        (i for i, line in enumerate(lines) if 'depends_on "python@3.11"' in line), None
    )
    assert python_idx is not None, "depends_on python@3.11 not found"
    assert python_idx == llama_idx + 1, (
        f"python@3.11 depends_on must be on line immediately after llama.cpp. "
        f"llama.cpp at line {llama_idx + 1}, python@3.11 at line {python_idx + 1}"
    )


def test_formula_has_install_block(formula_text):
    """Formula must have a def install block."""
    assert "def install" in formula_text
    assert re.search(r"def install\s*\n", formula_text)


def test_formula_has_caveats_block(formula_text):
    """Formula must have a def caveats heredoc."""
    assert "def caveats" in formula_text
    assert "<<~EOS" in formula_text


def test_formula_has_service_block(formula_text):
    """Formula must have a service block for brew services."""
    assert "service do" in formula_text


def test_formula_has_test_block(formula_text):
    """Formula must have a test do block."""
    assert "test do" in formula_text


def test_formula_heredoc_delimiters_match(formula_text):
    """The caveats EOS heredoc must have matching opening and closing delimiters."""
    open_count = formula_text.count("<<~EOS")
    close_count = len(re.findall(r"^\s*EOS\s*$", formula_text, re.MULTILINE))
    assert open_count == close_count, (
        f"Mismatched heredoc delimiters: {open_count} opening, {close_count} closing"
    )


# ── 2. Sidecar --version flag exits before dependency guard ───────────────────


def test_version_flag_present_in_sidecar(sidecar_text):
    """Sidecar must contain the --version / -V flag block."""
    assert '--version' in sidecar_text
    assert '-V' in sidecar_text


def test_version_block_precedes_dependency_guard(sidecar_text):
    """The version flag block must appear before _MISSING = [] in the source."""
    version_idx = sidecar_text.find('sys.argv[1] in ("--version", "-V")')
    missing_idx = sidecar_text.find("_MISSING = []")
    assert version_idx != -1, "Version flag block not found in sidecar"
    assert missing_idx != -1, "_MISSING dependency guard not found in sidecar"
    assert version_idx < missing_idx, (
        f"Version flag (offset {version_idx}) must precede _MISSING guard "
        f"(offset {missing_idx})"
    )


def test_version_flag_exits_0(sidecar_text):
    """Version flag block must call sys.exit(0)."""
    # Find the version flag block and confirm sys.exit(0) follows it.
    match = re.search(
        r'sys\.argv\[1\] in \("--version", "-V"\).*?sys\.exit\(0\)',
        sidecar_text,
        re.DOTALL,
    )
    assert match, "sys.exit(0) not found after the --version check"


def test_version_flag_prints_correct_string(sidecar_text):
    """Version flag block must print 'privacyclaw-slm-sidecar 0.1.0'."""
    assert 'print("privacyclaw-slm-sidecar 0.1.0")' in sidecar_text


def test_version_flag_runs_and_exits_without_deps():
    """
    Running the sidecar with --version must exit 0 and print the version string.
    Uses the system Python; fastapi/uvicorn are not required.
    """
    result = subprocess.run(
        [sys.executable, str(_SIDECAR), "--version"],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert result.returncode == 0, (
        f"Expected exit 0, got {result.returncode}. "
        f"stdout={result.stdout!r} stderr={result.stderr!r}"
    )
    assert "privacyclaw-slm-sidecar 0.1.0" in result.stdout


def test_version_flag_short_form_exits_without_deps():
    """
    Running the sidecar with -V must also exit 0 (short form support).
    """
    result = subprocess.run(
        [sys.executable, str(_SIDECAR), "-V"],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert result.returncode == 0, (
        f"Expected exit 0 for -V, got {result.returncode}. "
        f"stdout={result.stdout!r} stderr={result.stderr!r}"
    )
    assert "privacyclaw-slm-sidecar 0.1.0" in result.stdout


# ── 3. test-block assertion correctness ──────────────────────────────────────


def test_test_block_contains_sidecar_assertion(formula_text):
    """
    The sidecar smoke-test assertion must be inside the test do block,
    not after it, and must use the exact expected string.
    """
    # Find the test do block boundaries.
    test_start = formula_text.find("test do")
    assert test_start != -1, "test do block not found"

    # Find the matching 'end' for test do by counting do/end pairs.
    # We search from test_start forward.
    after_test = formula_text[test_start:]
    depth = 0
    test_end_offset = None
    for m in re.finditer(r'\b(do|end)\b', after_test):
        tok = m.group()
        if tok == "do":
            depth += 1
        else:
            depth -= 1
            if depth == 0:
                test_end_offset = m.end()
                break

    assert test_end_offset is not None, "Could not find closing end for test do block"
    test_block = after_test[:test_end_offset]

    assert 'privacyclaw-slm-sidecar 0.1.0' in test_block, (
        "Sidecar version string not found inside test do...end block"
    )
    assert 'assert_match' in test_block, (
        "assert_match not found inside test do...end block"
    )
    assert '--version' in test_block, (
        "--version flag not referenced inside test do...end block"
    )


def test_test_block_assertion_uses_assert_match(formula_text):
    """
    The sidecar test assertion must use assert_match (not assert_equal or other).
    """
    assert 'assert_match "privacyclaw-slm-sidecar 0.1.0"' in formula_text


def test_test_block_uses_bin_path(formula_text):
    """
    The sidecar test must invoke the binary via #{bin}/privacyclaw-slm-sidecar.
    """
    assert '#{bin}/privacyclaw-slm-sidecar --version' in formula_text


def test_sidecar_assertion_is_inside_test_block_not_after(formula_text):
    """
    Verify that 'privacyclaw-slm-sidecar 0.1.0' does not appear *after* the
    final 'end' of the test do block (i.e., it's not accidentally placed outside).
    """
    test_start = formula_text.find("test do")
    assert test_start != -1

    after_test = formula_text[test_start:]
    depth = 0
    test_end_abs = None
    for m in re.finditer(r'\b(do|end)\b', after_test):
        tok = m.group()
        if tok == "do":
            depth += 1
        else:
            depth -= 1
            if depth == 0:
                test_end_abs = test_start + m.end()
                break

    assert test_end_abs is not None
    after_test_block = formula_text[test_end_abs:]
    assert 'privacyclaw-slm-sidecar 0.1.0' not in after_test_block, (
        "Version assertion found AFTER the test do...end block — it must be inside"
    )


# ── 4. pydantic-core platform-specific block structure ───────────────────────


def test_pydantic_core_on_macos_block_present(formula_text):
    """Formula must have on_macos do block for pydantic-core."""
    assert "on_macos do" in formula_text


def test_pydantic_core_on_arm_block_present(formula_text):
    """Formula must have on_arm do block inside on_macos for pydantic-core."""
    assert "on_arm do" in formula_text


def test_pydantic_core_on_intel_block_present(formula_text):
    """Formula must have on_intel do block inside on_macos for pydantic-core."""
    assert "on_intel do" in formula_text


def test_pydantic_core_arm_wheel_is_arm64(formula_text):
    """The on_arm pydantic-core resource must reference an arm64 wheel."""
    # Find the on_arm block and extract the URL.
    on_arm_match = re.search(
        r'on_arm\s+do\s+(.*?)\s+end',
        formula_text,
        re.DOTALL,
    )
    assert on_arm_match, "on_arm do block not found"
    on_arm_block = on_arm_match.group(1)
    assert "arm64" in on_arm_block, (
        f"Expected arm64 in on_arm block. Got: {on_arm_block!r}"
    )
    assert "pydantic_core" in on_arm_block


def test_pydantic_core_intel_wheel_is_x86_64(formula_text):
    """The on_intel pydantic-core resource must reference an x86_64 wheel."""
    on_intel_match = re.search(
        r'on_intel\s+do\s+(.*?)\s+end',
        formula_text,
        re.DOTALL,
    )
    assert on_intel_match, "on_intel do block not found"
    on_intel_block = on_intel_match.group(1)
    assert "x86_64" in on_intel_block, (
        f"Expected x86_64 in on_intel block. Got: {on_intel_block!r}"
    )
    assert "pydantic_core" in on_intel_block


def test_pydantic_core_blocks_are_nested_inside_on_macos(formula_text):
    """on_arm and on_intel blocks must be nested inside on_macos do."""
    on_macos_match = re.search(
        r'on_macos\s+do\s+(.*?)\nend',
        formula_text,
        re.DOTALL,
    )
    assert on_macos_match, "on_macos do block not found"
    on_macos_block = on_macos_match.group(1)
    assert "on_arm do" in on_macos_block, "on_arm do must be inside on_macos do"
    assert "on_intel do" in on_macos_block, "on_intel do must be inside on_macos do"


def test_pydantic_core_appears_exactly_twice(formula_text):
    """pydantic-core resource must appear exactly twice (once per arch)."""
    count = formula_text.count('resource "pydantic-core"')
    assert count == 2, (
        f"Expected exactly 2 pydantic-core resource blocks, found {count}"
    )


def test_pydantic_core_arm_wheel_uses_cp311(formula_text):
    """The arm64 pydantic-core wheel must target CPython 3.11 (cp311)."""
    on_arm_match = re.search(r'on_arm\s+do\s+(.*?)\s+end', formula_text, re.DOTALL)
    assert on_arm_match
    assert "cp311" in on_arm_match.group(1)


def test_pydantic_core_intel_wheel_uses_cp311(formula_text):
    """The x86_64 pydantic-core wheel must target CPython 3.11 (cp311)."""
    on_intel_match = re.search(r'on_intel\s+do\s+(.*?)\s+end', formula_text, re.DOTALL)
    assert on_intel_match
    assert "cp311" in on_intel_match.group(1)


def test_typing_extensions_appears_exactly_once(formula_text):
    """typing-extensions resource block must appear exactly once (deduplication)."""
    count = formula_text.count('resource "typing-extensions"')
    assert count == 1, (
        f"Expected exactly 1 typing-extensions resource block, found {count}"
    )


# ── 5. install block: inreplace regex correctness ─────────────────────────────


def test_install_block_has_virtualenv_install(formula_text):
    """install block must call virtualenv_install_with_resources."""
    assert "virtualenv_install_with_resources" in formula_text


def test_install_block_uses_python311_for_virtualenv(formula_text):
    """virtualenv_install_with_resources must specify python@3.11."""
    assert 'virtualenv_install_with_resources using: "python@3.11"' in formula_text


def test_install_block_installs_privacyclaw_binary_first(formula_text):
    """bin.install privacyclaw must appear before virtualenv_install_with_resources."""
    privacyclaw_idx = formula_text.find('bin.install "privacyclaw"')
    virtualenv_idx = formula_text.find("virtualenv_install_with_resources")
    assert privacyclaw_idx != -1, 'bin.install "privacyclaw" not found'
    assert virtualenv_idx != -1, "virtualenv_install_with_resources not found"
    assert privacyclaw_idx < virtualenv_idx, (
        "bin.install privacyclaw must precede virtualenv_install_with_resources"
    )


def test_install_block_virtualenv_before_sidecar_bin_install(formula_text):
    """virtualenv_install_with_resources must appear before bin.install privacyclaw-slm-sidecar."""
    virtualenv_idx = formula_text.find("virtualenv_install_with_resources")
    sidecar_bin_idx = formula_text.find('bin.install "privacyclaw-slm-sidecar"')
    assert virtualenv_idx != -1, "virtualenv_install_with_resources not found"
    assert sidecar_bin_idx != -1, 'bin.install "privacyclaw-slm-sidecar" not found'
    assert virtualenv_idx < sidecar_bin_idx, (
        "virtualenv_install_with_resources must precede bin.install privacyclaw-slm-sidecar"
    )


def test_inreplace_uses_backslash_A_anchor(formula_text):
    r"""
    inreplace regex must use \A (start-of-file) anchor, not ^ (start-of-line).
    Using ^ would match any shebang-like line in the middle of the file.
    """
    # The inreplace call must contain /\A ... / regex literal.
    assert r"/\A#!" in formula_text, (
        r"inreplace regex must use \A anchor (start-of-file), not ^ (start-of-line). "
        r"Expected /\A#!.../ in formula."
    )


def test_inreplace_does_not_use_caret_anchor(formula_text):
    r"""
    inreplace must NOT use ^ as the anchor (would match any line, not just
    the first line of the file).
    """
    # Check the inreplace call specifically.
    inreplace_match = re.search(r'inreplace\s+bin/"privacyclaw-slm-sidecar".*', formula_text)
    assert inreplace_match, "inreplace call for privacyclaw-slm-sidecar not found"
    inreplace_line = inreplace_match.group()
    # The regex in the inreplace call must not start with /^
    assert not re.search(r'/\^', inreplace_line), (
        "inreplace regex must not use ^ anchor — use \\A instead"
    )


def test_inreplace_uses_opt_libexec(formula_text):
    """inreplace replacement string must reference opt_libexec, not venv.root."""
    assert "opt_libexec" in formula_text, (
        "inreplace replacement must use opt_libexec (not venv.root)"
    )
    assert "venv.root" not in formula_text, (
        "inreplace must not reference venv.root — use opt_libexec"
    )


def test_inreplace_targets_sidecar_in_bin(formula_text):
    """inreplace must target bin/privacyclaw-slm-sidecar (path inside bin)."""
    assert 'inreplace bin/"privacyclaw-slm-sidecar"' in formula_text


def test_inreplace_replacement_ends_with_newline(formula_text):
    r"""
    The inreplace replacement string must end with \n to preserve the newline
    after the shebang line.
    """
    assert r'bin/python3\n"' in formula_text, (
        r"inreplace replacement must end with \n to preserve the shebang newline"
    )


# ── Caveats: T3 SLM sidecar section ──────────────────────────────────────────


def test_caveats_contains_t3_section(formula_text):
    """caveats must contain the T3 PII pipeline section (Task 5.1)."""
    assert "T3 PII pipeline (SLM sidecar):" in formula_text


def test_caveats_mentions_smollm2(formula_text):
    """caveats must mention smollm2-135m model (Task 5.1)."""
    assert "smollm2-135m" in formula_text


def test_caveats_mentions_models_install(formula_text):
    """caveats must mention the privacyclaw models install command."""
    assert "privacyclaw models install" in formula_text


def test_caveats_mentions_sidecar_version_command(formula_text):
    """caveats must show the --version usage example for the sidecar."""
    assert "privacyclaw-slm-sidecar --version" in formula_text


def test_caveats_mentions_sidecar_port_env(formula_text):
    """caveats must show the SIDECAR_PORT usage example."""
    assert "SIDECAR_PORT=16442 privacyclaw-slm-sidecar" in formula_text


# ── Resource blocks: all 14 non-pydantic-core packages present ───────────────


@pytest.mark.parametrize("package", [
    "fastapi",
    "uvicorn",
    "httpx",
    "pydantic",
    "starlette",
    "anyio",
    "sniffio",
    "httpcore",
    "h11",
    "certifi",
    "idna",
    "click",
    "annotated-types",
    "typing-extensions",
])
def test_resource_block_present(formula_text, package):
    """Each required pip package must have a resource block in the formula."""
    assert f'resource "{package}"' in formula_text, (
        f"Missing resource block for '{package}'"
    )


@pytest.mark.parametrize("package", [
    "fastapi",
    "uvicorn",
    "httpx",
    "pydantic",
    "starlette",
    "anyio",
    "sniffio",
    "httpcore",
    "h11",
    "certifi",
    "idna",
    "click",
    "annotated-types",
    "typing-extensions",
])
def test_resource_block_has_url_and_sha256(formula_text, package):
    """Each resource block must have both url and sha256 fields."""
    # Extract the resource block for this package.
    pattern = rf'resource "{re.escape(package)}"\s+do\s+(.*?)\s+end'
    match = re.search(pattern, formula_text, re.DOTALL)
    assert match, f"resource block for '{package}' not found or malformed"
    block = match.group(1)
    assert "url " in block, f"url field missing from resource '{package}'"
    assert "sha256 " in block, f"sha256 field missing from resource '{package}'"


@pytest.mark.parametrize("package", [
    "fastapi",
    "uvicorn",
    "httpx",
    "pydantic",
    "starlette",
    "anyio",
    "sniffio",
    "httpcore",
    "h11",
    "certifi",
    "idna",
    "click",
    "annotated-types",
    "typing-extensions",
])
def test_resource_url_is_pythonhosted(formula_text, package):
    """Each resource URL must point to files.pythonhosted.org."""
    pattern = rf'resource "{re.escape(package)}"\s+do\s+(.*?)\s+end'
    match = re.search(pattern, formula_text, re.DOTALL)
    assert match
    block = match.group(1)
    assert "files.pythonhosted.org" in block, (
        f"resource '{package}' URL must point to files.pythonhosted.org"
    )
