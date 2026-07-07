def median(xs):
    """Return the median of a non-empty list."""
    s = sorted(xs)
    n = len(s)
    mid = n // 2
    if n % 2 == 0:
        return (s[mid - 1] + s[mid]) / 2
    return s[mid]


def mean(xs):
    """Return the arithmetic mean of a non-empty list."""
    return sum(xs) / len(xs)


def test_median_even():
    assert median([1, 2, 3, 4]) == 2.5


def test_median_odd():
    assert median([1, 2, 3, 4, 5]) == 3


def test_mean():
    assert mean([1, 2, 3]) == 2
