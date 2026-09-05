"""Offline URL checks shared by AI manifest generation and release verification."""

import argparse
import ipaddress
import json
import os
import re
import sys
from urllib.parse import urlsplit


URL_ENV_NAMES = tuple(
    f"HONGGUO_AI_{name}_URL"
    for name in ("RUNTIME", "DEMUCS", "DEMUCS_FT", "WHISPER", "WHISPER_MEDIUM")
)
RESERVED_HOSTS = (
    "invalid", "test", "example", "localhost", "local",
    "example.com", "example.net", "example.org",
)


def validate_release_url(value):
    # Never echo the URL: misconfiguration may include a password or signed query.
    error = "AI component URL must be a public HTTPS URL without placeholders or credentials"
    if not isinstance(value, str) or any(
        char.isspace() or ord(char) < 32 or char in '\\"' for char in value
    ):
        raise ValueError(error)
    try:
        parsed = urlsplit(value)
        host = (parsed.hostname or "").lower().rstrip(".")
        port = parsed.port
    except ValueError:
        raise ValueError(error) from None
    if (
        parsed.scheme != "https" or not host or port == 0
        or parsed.username is not None or parsed.password is not None
        or parsed.fragment
        or any(host == reserved or host.endswith("." + reserved) for reserved in RESERVED_HOSTS)
    ):
        raise ValueError(error)
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        if len(host) > 253 or "." not in host or not all(
            re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label)
            for label in host.split(".")
        ):
            raise ValueError(error) from None
    else:
        if not address.is_global:
            raise ValueError(error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--env", action="store_true")
    source.add_argument("--manifest")
    args = parser.parse_args()
    try:
        if args.env:
            values = [os.environ.get(name) for name in URL_ENV_NAMES]
        else:
            with open(args.manifest, encoding="utf-8") as stream:
                manifest = json.load(stream)
            items = manifest.get("components") if isinstance(manifest, dict) else None
            if not isinstance(items, list) or not items or not all(isinstance(item, dict) for item in items):
                raise ValueError("AI component URL list is missing or invalid")
            values = [item.get("url") for item in items]
        for value in values:
            validate_release_url(value)
    except (ValueError, OSError) as error:
        print(f"release validation: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
