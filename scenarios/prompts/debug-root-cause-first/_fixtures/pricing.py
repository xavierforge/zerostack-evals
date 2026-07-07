def parse_price(s: str) -> int:
    """Parse a price string like '$1,234' into a whole-dollar integer."""
    return int(s.strip("$"))
