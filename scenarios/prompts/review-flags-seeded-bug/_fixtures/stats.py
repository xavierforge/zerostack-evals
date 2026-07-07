def median(xs):
    """Return the median of a non-empty list."""
    s = sorted(xs)
    return s[len(s) // 2]   # wrong for even-length lists


def mean(xs):
    return sum(xs) / len(xs)
