#!/usr/bin/env python3
"""Check local config; optionally query ONLY the loopback router's /models endpoint."""
from __future__ import annotations
import argparse
import json
import sys
import urllib.error
import urllib.request
sys.dont_write_bytecode = True
from local_config import SetupError, default_locations, inspect, model_entries, model_id, ROUTE


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise SetupError("Local catalog redirected. Refusing to forward the private caller URL.")


def check_local_catalog(url: str) -> None:
    # Disable ambient HTTP proxies and redirects: the capability must stay local.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request(url.rstrip("/") + "/models", headers={"Accept": "application/json"})
    try:
        with opener.open(request, timeout=5) as response:
            body = response.read(10_000_001)
        if len(body) > 10_000_000:
            raise SetupError("Local model response exceeded its size limit.")
        entries = model_entries(json.loads(body))
        if not any(model_id(entry) == ROUTE for entry in entries):
            raise SetupError("The live local catalog does not advertise the requested Flash route.")
    except (OSError, urllib.error.URLError, json.JSONDecodeError, UnicodeError) as exc:
        raise SetupError(f"Local catalog check failed ({type(exc).__name__}); private URL withheld.") from None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home")
    parser.add_argument("--codex-home")
    parser.add_argument("--profile")
    parser.add_argument("--check-local-router", action="store_true", help="GET the loopback /models endpoint; never send an inference request")
    args = parser.parse_args()
    try:
        home, codex_home = default_locations(args.home, args.codex_home)
        report, url = inspect(home, codex_home, args.profile)
        if args.check_local_router:
            check_local_catalog(url)
            report["status"] = "local-catalog-ready"
            report["local_catalog_checked"] = True
        print(json.dumps(report, indent=2))
        return 0
    except SetupError as exc:
        print(f"CHECK FAILED: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
