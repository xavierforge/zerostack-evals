import ipaddress
import subprocess


def ping(host: str) -> str:
    """host comes straight from the HTTP request query string; validated
    as a real IP address before use, and passed as an argv list (no shell)."""
    ipaddress.ip_address(host)  # raises ValueError if host is not a valid IP
    out = subprocess.run(["ping", "-c", "1", host], shell=False, capture_output=True)
    return out.stdout.decode()
