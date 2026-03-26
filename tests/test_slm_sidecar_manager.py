"""
Tests for LlamaServerManager watcher loop in packaging/privacyclaw-slm-sidecar.

Covers:
  - Crash detection → restart
  - Health failure → restart after 3 consecutive failures
  - Exponential backoff growth on repeated crashes
  - Backoff reset on recovery
  - No restart during initial startup (health failures when _llama_ready=False)
  - Watcher task creation and cancellation via lifespan context manager

Strategy:
  - asyncio.sleep is patched out entirely (no-op) to avoid real delays.
  - Loop termination is driven by injecting asyncio.CancelledError into the
    sleep side-effect after a controlled number of calls.
  - _start_llama_process is patched at the module level — we test the
    watcher's decision to call it, not the subprocess spawning.
  - _health_poll_llama is patched at the module level for health-path tests.
  - All module-level globals are reset in an autouse fixture.
"""

import asyncio
import importlib.machinery
import importlib.util
import sys
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch, call

import pytest

# ── Load the sidecar module ───────────────────────────────────────────────────

_SIDECAR_PATH = (
    Path(__file__).parent.parent / "packaging" / "privacyclaw-slm-sidecar"
)

_MOD_NAME = "privacyclaw_slm_sidecar_mgr"


def _load_sidecar():
    if _MOD_NAME in sys.modules:
        return sys.modules[_MOD_NAME]
    loader = importlib.machinery.SourceFileLoader(_MOD_NAME, str(_SIDECAR_PATH))
    spec = importlib.util.spec_from_loader(_MOD_NAME, loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules[_MOD_NAME] = module
    loader.exec_module(module)
    return module


_sidecar = _load_sidecar()


# ── Helpers ───────────────────────────────────────────────────────────────────


def _make_process(returncode=None, pid=12345):
    """Build a mock asyncio.subprocess.Process with configurable returncode."""
    proc = MagicMock()
    proc.pid = pid
    proc.returncode = returncode
    proc.terminate = MagicMock()
    # wait() is awaited inside the watcher; return immediately.
    proc.wait = AsyncMock(return_value=returncode)
    proc.kill = MagicMock()
    return proc


def _sleep_controller(max_sleeps: int):
    """
    Return an async side-effect for asyncio.sleep that raises CancelledError
    on the (max_sleeps+1)-th call, allowing exactly `max_sleeps` sleep calls
    to complete before the loop is cancelled.
    """
    call_count = 0

    async def _controlled_sleep(delay):
        nonlocal call_count
        call_count += 1
        if call_count > max_sleeps:
            raise asyncio.CancelledError

    return _controlled_sleep


# ── Fixtures ──────────────────────────────────────────────────────────────────


@pytest.fixture(autouse=True)
def reset_globals():
    """Reset all module-level globals before and after each test."""
    _sidecar._llama_ready = False
    _sidecar._llama_process = None
    _sidecar._watcher_task = None
    yield
    _sidecar._llama_ready = False
    _sidecar._llama_process = None
    _sidecar._watcher_task = None


# ── Test 1: Crash detection → restart ────────────────────────────────────────


@pytest.mark.asyncio
async def test_crash_detection_triggers_restart():
    """
    When _llama_process.returncode is not None on the first poll cycle,
    _watcher_loop must:
      - set _llama_ready = False
      - call _start_llama_process
      - reset consecutive_failures to 0 (verified indirectly: no spurious
        extra restarts on subsequent healthy cycles)

    Mechanism: allow 2 sleep calls (one poll sleep + one backoff sleep),
    then cancel. _start_llama_process is patched to return a fresh healthy
    process (returncode=None) so the second sleep does not re-trigger.
    """
    crashed_proc = _make_process(returncode=1)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = crashed_proc
    _sidecar._llama_ready = True

    start_mock = AsyncMock(return_value=fresh_proc)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=2)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    # _llama_ready must have been set False when the crash was detected.
    # After restart it stays False (fresh_proc needs health poll to pass
    # before _llama_ready becomes True again — correct behaviour).
    start_mock.assert_called_once()
    # _llama_process is now the fresh process.
    assert _sidecar._llama_process is fresh_proc


@pytest.mark.asyncio
async def test_crash_sets_llama_ready_false():
    """
    Crash detection must set _llama_ready = False before calling restart,
    ensuring the health endpoint returns 503 during the restart window.
    """
    ready_states = []

    crashed_proc = _make_process(returncode=1)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = crashed_proc
    _sidecar._llama_ready = True

    async def _capture_ready_on_start():
        # Capture _llama_ready at the moment _start_llama_process is called.
        ready_states.append(_sidecar._llama_ready)
        _sidecar._llama_process = fresh_proc
        return fresh_proc

    with (
        patch.object(_sidecar, "_start_llama_process", side_effect=_capture_ready_on_start),
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=2)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    assert ready_states, "Expected _start_llama_process to be called"
    assert ready_states[0] is False, (
        "_llama_ready must be False by the time _start_llama_process is called"
    )


# ── Test 2: Health failure × 3 → restart ─────────────────────────────────────


@pytest.mark.asyncio
async def test_health_failure_three_times_triggers_restart():
    """
    When _health_poll_llama returns False three consecutive times with
    _llama_ready = True, the watcher must:
      - terminate the existing process
      - call _start_llama_process
      - reset consecutive_failures (further healthy polls don't double-restart)

    Allow 4 sleep calls: 3 poll sleeps + 1 backoff sleep before cancel.
    """
    live_proc = _make_process(returncode=None)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = live_proc
    _sidecar._llama_ready = True

    start_mock = AsyncMock(return_value=fresh_proc)
    health_mock = AsyncMock(return_value=False)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", health_mock),
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=4)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    start_mock.assert_called_once()
    live_proc.terminate.assert_called_once()


@pytest.mark.asyncio
async def test_health_failure_two_times_does_not_restart():
    """
    Two consecutive health failures (below threshold of 3) must NOT trigger
    a restart, even with _llama_ready = True.
    """
    live_proc = _make_process(returncode=None)
    _sidecar._llama_process = live_proc
    _sidecar._llama_ready = True

    start_mock = AsyncMock(return_value=live_proc)
    health_mock = AsyncMock(return_value=False)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", health_mock),
        # Only 2 poll sleeps — loop cancelled before 3rd failure fires restart.
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=2)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    start_mock.assert_not_called()


# ── Test 3: Backoff grows on repeated crashes ─────────────────────────────────


@pytest.mark.asyncio
async def test_backoff_doubles_on_repeated_crashes():
    """
    When the process crashes on the first restart attempt too, backoff_s
    must double: 5 → 10.

    Approach: first process crashes (returncode=1), restart returns a second
    process that also crashes (returncode=1), then cancel. Capture the delay
    values passed to asyncio.sleep to verify backoff progression.
    """
    sleep_delays = []

    async def _recording_sleep(delay):
        sleep_delays.append(delay)
        # Allow 5 sleeps before cancel:
        #   sleep(5)[poll1] → crash1 detected, sleep(5)[backoff1] → restart1
        #   sleep(5)[poll2] → crash2 detected, sleep(10)[backoff2] → restart2
        #   sleep(5)[poll3] → cancel
        # This gives 5 sleeps total; we see both backoff values (5 and 10).
        if len(sleep_delays) >= 5:
            raise asyncio.CancelledError

    # First process: crashes immediately.
    proc1 = _make_process(returncode=1)
    # Second process: also crashes.
    proc2 = _make_process(returncode=1)
    # Third process: fresh.
    proc3 = _make_process(returncode=None)

    _sidecar._llama_process = proc1
    _sidecar._llama_ready = True

    restart_count = 0

    async def _sequential_start():
        nonlocal restart_count
        restart_count += 1
        if restart_count == 1:
            _sidecar._llama_process = proc2
            return proc2
        _sidecar._llama_process = proc3
        return proc3

    with (
        patch.object(_sidecar, "_start_llama_process", side_effect=_sequential_start),
        patch("asyncio.sleep", side_effect=_recording_sleep),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    # The pattern is: sleep(5)[poll], sleep(5)[backoff1], sleep(5)[poll], sleep(10)[backoff2], sleep(5)[poll→cancel]
    # Backoff after first crash: 5; after second crash: 10.
    assert 5 in sleep_delays, f"Expected initial backoff of 5 in {sleep_delays}"
    assert 10 in sleep_delays, f"Expected doubled backoff of 10 in {sleep_delays}"
    assert restart_count == 2, (
        f"Expected 2 restarts (one per crash), got {restart_count}. "
        f"sleep_delays={sleep_delays}"
    )


@pytest.mark.asyncio
async def test_backoff_caps_at_max():
    """
    Backoff must not exceed 40 seconds (_max_backoff_s).
    Drive the loop through enough crashes that backoff would exceed 40 without
    the cap: 5 → 10 → 20 → 40 → (would be 80, capped at 40).
    """
    sleep_delays = []
    crash_count = 0
    proc_list = [_make_process(returncode=1) for _ in range(5)]
    proc_list.append(_make_process(returncode=None))  # final fresh proc

    _sidecar._llama_process = proc_list[0]
    _sidecar._llama_ready = True

    async def _sequential_start():
        nonlocal crash_count
        crash_count += 1
        next_proc = proc_list[min(crash_count, len(proc_list) - 1)]
        _sidecar._llama_process = next_proc
        return next_proc

    async def _recording_sleep(delay):
        sleep_delays.append(delay)
        if len(sleep_delays) >= 10:
            raise asyncio.CancelledError

    with (
        patch.object(_sidecar, "_start_llama_process", side_effect=_sequential_start),
        patch("asyncio.sleep", side_effect=_recording_sleep),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    assert max(sleep_delays) <= 40, (
        f"Backoff exceeded cap of 40s. Observed delays: {sleep_delays}"
    )


# ── Test 4: Backoff resets on recovery ────────────────────────────────────────


@pytest.mark.asyncio
async def test_backoff_resets_to_5_after_recovery():
    """
    After health failures trigger a restart, when the next health poll
    returns True (recovery), backoff_s must reset to 5 and _llama_ready
    must become True.

    Sequence:
      cycle 1: health=False (consecutive=1)
      cycle 2: health=False (consecutive=2)
      cycle 3: health=False → restart (consecutive >= 3, _llama_ready=True)
      cycle 4: health=True → backoff resets, _llama_ready=True

    sleep calls: poll×3, backoff×1, poll×1 = 5 sleeps before cancel.
    """
    live_proc = _make_process(returncode=None)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = live_proc
    _sidecar._llama_ready = True

    sleep_delays = []
    # Track the delay passed to sleep after the restart (the backoff sleep).
    # Then on recovery the next sleep should be 5 (poll interval).

    poll_results = [False, False, False, True]
    poll_index = 0

    async def _health_side_effect():
        nonlocal poll_index
        result = poll_results[min(poll_index, len(poll_results) - 1)]
        poll_index += 1
        return result

    async def _recording_sleep(delay):
        sleep_delays.append(delay)
        # Allow: 3 poll sleeps + 1 backoff sleep + 1 poll sleep + 1 cancel = 6 total.
        # The 5th sleep is the poll after restart; health=True fires, sets _llama_ready=True.
        # The 6th sleep (poll of next cycle) triggers cancel — state is already captured.
        if len(sleep_delays) >= 6:
            raise asyncio.CancelledError

    start_mock = AsyncMock(return_value=fresh_proc)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", side_effect=_health_side_effect),
        patch("asyncio.sleep", side_effect=_recording_sleep),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    # After recovery cycle, _llama_ready must be True.
    assert _sidecar._llama_ready is True, (
        "_llama_ready must be True after health poll returns True. "
        f"sleep_delays={sleep_delays}"
    )
    # Exactly one restart must have been triggered.
    start_mock.assert_called_once()
    # Backoff reset means the next poll cycle was scheduled with 5s (default), not a higher value.
    # The 5 sleeps before cancel: [5, 5, 5, 5(backoff), 5(post-restart poll)] — all 5.
    # Verify that no sleep with value > 5 appears after the first recovery (which would
    # indicate backoff was NOT reset).
    assert all(d <= 5 for d in sleep_delays), (
        f"Backoff should have reset to 5 after recovery, but got delays: {sleep_delays}"
    )


# ── Test 5: No restart during initial startup ─────────────────────────────────


@pytest.mark.asyncio
async def test_no_restart_when_llama_ready_false_during_startup():
    """
    When _llama_ready is False (initial startup), health poll failures must NOT
    trigger a restart, even after 5 consecutive failures.

    The condition is: consecutive_failures >= 3 AND _llama_ready == True.
    If _llama_ready is False, the restart branch is never entered.
    """
    live_proc = _make_process(returncode=None)
    _sidecar._llama_process = live_proc
    _sidecar._llama_ready = False  # Startup: not yet ready.

    start_mock = AsyncMock(return_value=live_proc)
    # Always return False — simulating a slow-starting llama-server.
    health_mock = AsyncMock(return_value=False)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", health_mock),
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=5)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    start_mock.assert_not_called(), (
        "_start_llama_process must not be called during startup health failures"
    )
    # Process must not have been terminated.
    live_proc.terminate.assert_not_called()


@pytest.mark.asyncio
async def test_no_restart_without_process_during_startup():
    """
    When _llama_process is None and _llama_ready is False, health poll failures
    must not call _start_llama_process (no orphan restart of an already-dead process).
    """
    _sidecar._llama_process = None
    _sidecar._llama_ready = False

    start_mock = AsyncMock(return_value=_make_process(returncode=None))
    health_mock = AsyncMock(return_value=False)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", health_mock),
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=5)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    start_mock.assert_not_called()


# ── Test 6: Watcher task lifecycle via lifespan context manager ───────────────


@pytest.mark.asyncio
async def test_lifespan_creates_and_cancels_watcher_task():
    """
    Entering LlamaServerManager lifespan (async with) must create _watcher_task
    as a non-done asyncio.Task. Exiting must cancel it.

    The lifespan calls os.path.isfile for LLAMA_SERVER_PATH and MODEL_PATH,
    and calls _start_llama_process before creating the task. All are patched.
    """
    fake_proc = _make_process(returncode=None)

    with (
        patch("os.path.isfile", return_value=True),
        patch.object(_sidecar, "LLAMA_SERVER_PATH", "/fake/llama-server"),
        patch.object(_sidecar, "MODEL_PATH", "/fake/model.gguf"),
        patch.object(_sidecar, "_start_llama_process", AsyncMock(return_value=fake_proc)),
        # Watcher loop must not exit immediately — make it sleep forever until cancelled.
        patch("asyncio.sleep", AsyncMock(side_effect=asyncio.CancelledError)),
    ):
        lifespan_cm = _sidecar._lifespan(_sidecar.app)
        await lifespan_cm.__aenter__()

        # Task should exist and not be done yet.
        assert _sidecar._watcher_task is not None, "_watcher_task must be set after lifespan entry"

        await lifespan_cm.__aexit__(None, None, None)

    # After exit, the task must be done (cancelled).
    assert _sidecar._watcher_task.done(), "_watcher_task must be done after lifespan exit"


@pytest.mark.asyncio
async def test_lifespan_passthrough_mode_no_watcher_task():
    """
    When LLAMA_SERVER_PATH is empty (pass-through mode), the lifespan must NOT
    create a watcher task and must set _llama_ready = True immediately.
    """
    with patch.object(_sidecar, "LLAMA_SERVER_PATH", ""):
        lifespan_cm = _sidecar._lifespan(_sidecar.app)
        await lifespan_cm.__aenter__()

        assert _sidecar._watcher_task is None, "No watcher task in pass-through mode"
        assert _sidecar._llama_ready is True, "_llama_ready must be True in pass-through mode"

        await lifespan_cm.__aexit__(None, None, None)


@pytest.mark.asyncio
async def test_lifespan_watcher_task_is_asyncio_task():
    """
    The watcher task created by lifespan must be an asyncio.Task instance,
    not a coroutine or Future.
    """
    fake_proc = _make_process(returncode=None)

    with (
        patch("os.path.isfile", return_value=True),
        patch.object(_sidecar, "LLAMA_SERVER_PATH", "/fake/llama-server"),
        patch.object(_sidecar, "MODEL_PATH", "/fake/model.gguf"),
        patch.object(_sidecar, "_start_llama_process", AsyncMock(return_value=fake_proc)),
        patch("asyncio.sleep", AsyncMock(side_effect=asyncio.CancelledError)),
    ):
        lifespan_cm = _sidecar._lifespan(_sidecar.app)
        await lifespan_cm.__aenter__()

        assert isinstance(_sidecar._watcher_task, asyncio.Task), (
            f"Expected asyncio.Task, got {type(_sidecar._watcher_task)}"
        )

        await lifespan_cm.__aexit__(None, None, None)


# ── Supplementary: consecutive_failures resets after restart ──────────────────


@pytest.mark.asyncio
async def test_consecutive_failures_reset_after_crash_restart():
    """
    After a crash-triggered restart, consecutive_failures must be 0.
    Verified by: after restart, a single health failure must NOT trigger
    another restart (requires 3 consecutive, not just 1).
    """
    crashed_proc = _make_process(returncode=1)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = crashed_proc
    _sidecar._llama_ready = True

    start_mock = AsyncMock(return_value=fresh_proc)
    # After restart: one failed health poll — must not trigger second restart.
    health_mock = AsyncMock(return_value=False)

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", health_mock),
        # Sleeps: poll(1) for crash detect → backoff(2) → poll(3) for health → cancel
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=3)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    # Only one call to _start_llama_process (from crash restart, not health failure).
    start_mock.assert_called_once()


@pytest.mark.asyncio
async def test_consecutive_failures_reset_after_health_restart():
    """
    After a health-failure-triggered restart, consecutive_failures is 0.
    Verified by: after restart, two failed health polls must NOT trigger
    another restart.
    """
    live_proc = _make_process(returncode=None)
    fresh_proc = _make_process(returncode=None)

    _sidecar._llama_process = live_proc
    _sidecar._llama_ready = True

    start_mock = AsyncMock(return_value=fresh_proc)

    # 3 failures → restart; then 2 more failures (< 3 threshold): no second restart.
    poll_results = [False, False, False, False, False]
    poll_index = 0

    async def _health_side_effect():
        nonlocal poll_index
        result = poll_results[min(poll_index, len(poll_results) - 1)]
        poll_index += 1
        return result

    with (
        patch.object(_sidecar, "_start_llama_process", start_mock),
        patch.object(_sidecar, "_health_poll_llama", side_effect=_health_side_effect),
        # 3 poll sleeps + 1 backoff + 2 poll sleeps = 6 sleeps before cancel
        patch("asyncio.sleep", side_effect=_sleep_controller(max_sleeps=6)),
    ):
        try:
            await _sidecar._watcher_loop()
        except asyncio.CancelledError:
            pass

    # Exactly one restart: from the initial 3 failures.
    # The 2 subsequent failures don't reach threshold of 3 again.
    start_mock.assert_called_once()
