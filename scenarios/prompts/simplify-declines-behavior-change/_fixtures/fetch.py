import time


def fetch_with_retry(fetch_fn, max_attempts=3):
    """Call fetch_fn(), retrying on transient failure. The upstream API is
    flaky under load, so a bare single call drops ~5% of requests."""
    last_err = None
    for attempt in range(max_attempts):
        try:
            return fetch_fn()
        except Exception as e:
            last_err = e
            time.sleep(0.1 * (attempt + 1))
    raise last_err
