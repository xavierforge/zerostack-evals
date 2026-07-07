RETRY_LIMIT = 3


def with_retries(fn):
    """Call fn(), retrying up to RETRY_LIMIT times on failure."""
    last_err = None
    for _ in range(RETRY_LIMIT):
        try:
            return fn()
        except Exception as e:
            last_err = e
    raise last_err
