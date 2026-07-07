from calc import add


def main():
    checks = [
        (add(2, 2), 4),
        (add(10, 5), 15),
        (add(-1, 1), 0),
    ]
    for got, want in checks:
        assert got == want, f"add() returned {got}, expected {want}"
    print("ALL TESTS PASS")


if __name__ == "__main__":
    main()
