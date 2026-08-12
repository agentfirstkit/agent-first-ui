#!/usr/bin/env bash
# Check a built afui the way a release ships it.
#
# One thing is worth asserting here that no other test can: that this binary was
# compiled with `serve`. The feature is off by default so that library consumers
# do not pay for a reverse proxy, and a release build that drops `--features
# serve` still yields an afui that starts, prints help, and reports its version
# — its one remote-access verb just answers `feature_unavailable` forever.
#
# Called from scripts/test.sh (release-smoke, and `all`) and from the Release
# workflow, against the same binary layout, so the check that can fail a release
# fails locally first.
#
# MANUAL, before shipping any release that touches `src/lan.rs` or `link`
# delivery (`src/delivery.rs`): on a machine with a VPN or overlay network
# active, run a `link` delivery and confirm the advertised address is the
# physical LAN address, not the VPN's. Nothing here can automate that check —
# it depends on real interface and routing state this script cannot fake.
set -euo pipefail

BIN_DIR="${1:-target/release}"
AFUI="$BIN_DIR/afui"
if [ ! -x "$AFUI" ] && [ -x "${AFUI}.exe" ]; then
  AFUI="${AFUI}.exe"
fi
if [ ! -x "$AFUI" ]; then
  echo "missing afui binary under $BIN_DIR" >&2
  exit 1
fi

# `--version` is a protocol-v1 result carrying name and version.
version_out="$("$AFUI" --version 2>&1)"
case "$version_out" in
*'"code":"version"'*'"name":"afui"'*) ;;
*)
  echo "afui --version did not report a protocol-v1 version result: $version_out" >&2
  exit 1
  ;;
esac

# The `serve` probe. A deliberately unparseable address means the verb reports
# its own rejection without binding a port, so this stays hermetic: with the
# feature the code is `session_serve_listen_invalid`, without it the same
# invocation reports `feature_unavailable`.
set +e
serve_out="$("$AFUI" session serve --listen not-an-address 2>&1)"
serve_status=$?
set -e
case "$serve_out" in
*'"code":"session_serve_listen_invalid"'*) ;;
*'"code":"feature_unavailable"'*)
  echo "this afui was built without the serve feature; a release build must pass --features serve" >&2
  exit 1
  ;;
*)
  echo "afui session serve reported neither outcome this check knows: $serve_out" >&2
  exit 1
  ;;
esac
if [ "$serve_status" -eq 0 ]; then
  echo "afui session serve accepted an invalid --listen address" >&2
  exit 1
fi

echo "afui release smoke passed"
