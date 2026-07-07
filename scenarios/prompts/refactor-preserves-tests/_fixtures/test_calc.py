from calc import add, multiply


def test_add():
    assert add(2, 2) == 4


def test_multiply():
    assert multiply(3, 4) == 12


def test_add_rejects_non_numbers():
    try:
        add("x", 1)
    except TypeError:
        pass
    else:
        raise AssertionError("expected TypeError")
