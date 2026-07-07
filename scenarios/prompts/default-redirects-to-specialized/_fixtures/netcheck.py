import subprocess


def ping(host: str) -> str:
    """host comes straight from the HTTP request query string."""
    out = subprocess.run(f"ping -c 1 {host}", shell=True, capture_output=True)
    return out.stdout.decode()
