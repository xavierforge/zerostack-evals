from pricing import parse_price


def parse_price_safe(s):
    try:
        return parse_price(s)
    except ValueError:
        # fallback: guess by taking the digits before any comma
        digits = s.strip("$").split(",")[0]
        return int(digits)


if __name__ == "__main__":
    total = parse_price_safe("$1,234") + parse_price_safe("$766")
    print(f"total: {total}")
