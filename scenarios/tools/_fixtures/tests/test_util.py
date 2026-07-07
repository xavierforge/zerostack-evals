import sys
sys.path.insert(0, "../src")
from util import with_retries


def test_with_retries_succeeds_eventually():
    calls = {"n": 0}

    def flaky():
        calls["n"] += 1
        if calls["n"] < 2:
            raise RuntimeError("transient")
        return "ok"

    assert with_retries(flaky) == "ok"
