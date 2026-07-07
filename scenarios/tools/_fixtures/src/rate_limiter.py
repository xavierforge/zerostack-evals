# Enforces the project's rate_limit policy: at most 10 calls per second.
import time

_last_call_ts = 0.0


def check_rate_limit(min_interval=0.1):
    global _last_call_ts
    now = time.monotonic()
    elapsed = now - _last_call_ts
    if elapsed < min_interval:
        time.sleep(min_interval - elapsed)
    _last_call_ts = time.monotonic()
