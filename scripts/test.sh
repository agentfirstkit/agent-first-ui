#!/usr/bin/env bash
# Fixed test entrypoint for agent-first-ui.
#
# The single source of truth for how this spore is tested. CI, the monorepo
# driver (scripts/projects.sh test agent-first-ui), and the release gate all
# call this file, so a mode can never mean one thing locally and another in CI.
set -euo pipefail

_AFDATA_BASH_SOURCE="$("${AFDATA_BIN:-afdata}" shell bash)"
# shellcheck source=/dev/stdin
source /dev/stdin <<<"$_AFDATA_BASH_SOURCE"
unset _AFDATA_BASH_SOURCE

ROOT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_PATH"

afdata_args_begin "$0 [MODE]"
afdata_args_positional test_mode MODE "Test mode" optional
afdata_args_parse "$@"
MODE="${test_mode:-all}"
TEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/agent-first-ui-test.XXXXXX")"
trap 'rm -rf "$TEST_TMP"' EXIT

run_clippy() {
  afdata_run --quiet cargo clippy --all-targets "$@" -- -D warnings
}

# Every tier a consumer can ask for, because a tier nothing builds is a tier
# that compiles by accident. The guard tier is the one a Provider depends on
# from an ungated template loader, and it is also the one a default
# build would never notice breaking, because the default build has axum in it.
run_static() {
  # Parsing is not linting, and this ran neither until now. The pinned version
  # lives in the script itself, mirrored from the monorepo and compared byte for
  # byte there, so this spore and its siblings cannot drift onto different rules.
  "$ROOT_PATH/scripts/install-shellcheck.sh" --check "$ROOT_PATH"
  afdata_run --quiet cargo fmt --all --check
  run_clippy --no-default-features
  # `page` is orthogonal to `session` — a build that renders and checks a
  # page but binds no port. Nothing else in this gate compiles it.
  run_clippy --no-default-features --features page
  run_clippy
  # The shell and its reverse proxy are behind a feature, so the default build
  # never compiles them. A gate that only ran the default build would let the
  # one part of this crate that talks to a network rot.
  run_clippy --features serve
}

run_unit() {
  afdata_run --quiet cargo test --all-targets --no-default-features
  afdata_run --quiet cargo test --all-targets --no-default-features --features page
  afdata_run --quiet cargo test --all-targets
  afdata_run --quiet cargo test --doc
  afdata_run --quiet cargo test --doc --no-default-features --features page
  afdata_run --quiet cargo test --all-targets --features serve
  check_guard_tier_dependencies
}

# The guard tier's whole point is what it does *not* pull in.
#
# A Provider puts this crate in its ungated `[dependencies]` so its template
# loader can call the markup guard, and gates that build with a
# `--no-default-features` clippy profile. If a server crate ever leaks back into
# the featureless build, every such minimal build grows a runtime and nothing
# else here would say so. `page` gets the same check under its own flag: it
# adds minijinja and nothing session brings with it, so axum and tokio must
# still be absent from that tree too.
check_guard_tier_dependencies() {
  local tree dependency flags
  for flags in "--no-default-features" "--no-default-features --features page"; do
    # shellcheck disable=SC2086
    tree="$(cargo tree $flags --edges normal --prefix none)"
    for dependency in axum tokio tower hyper agent-first-data; do
      if [[ "$tree" == *"$dependency v"* ]]; then
        afdata_error unexpected_dependency \
          "$dependency must not be present in the '$flags' dependency tree" \
          "keep it behind the session feature" || :
        return 1
      fi
    done
  done
}

run_smoke() {
  local smoke_root="$TEST_TMP/workspace"
  local config_root="$TEST_TMP/config"
  mkdir -p "$smoke_root" "$config_root"
  afdata_run --quiet cargo build --bin afui
  (
    cd "$smoke_root"
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend init reviewer case_review \
      --frontend-id custom_review
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend check reviewer case_review
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend enable reviewer case_review
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend check reviewer case_review
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend diff reviewer case_review
    AFUI_CONFIG_DIR="$config_root" afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend disable reviewer case_review
    AFUI_CONFIG_DIR="$config_root" AFUI_SAFE_MODE=1 afdata_run --quiet \
      "$ROOT_PATH/target/debug/afui" frontend check reviewer case_review
  )
  run_serve_smoke "$config_root"
  # The same verb in its other shape, where every session gets an origin of its
  # own. `*.localhost` is loopback in every browser with no DNS at all, which is
  # what makes this the shape a person tries first and the one a gate can run.
  run_serve_smoke "$config_root" localhost
}

# `session serve` through the real binary: the CLI wiring, the credential it
# mints, and the refusal of anyone who does not have it.
#
# Deliberately self-contained. What a browser and a real Provider do with the
# page is covered by tests/session_serve.rs, which drives both in-process; this
# spore's gate must not need any consuming Provider to be built.
#
# Every request names its authority in a `Host` header and connects to loopback
# regardless, because a session's own name is not something this machine's
# resolver has ever been told about.
run_serve_smoke() {
  local config_root="$1"
  local session_origin_host="${2:-}"
  local binary="$ROOT_PATH/target/debug/afui"
  local log="$TEST_TMP/serve.jsonl"
  local serve_pid url host page wire status
  local -a extra_args=()

  [ -n "$session_origin_host" ] && extra_args=(--session-origin-host "$session_origin_host")

  afdata_run --quiet cargo build --bin afui --features serve
  # `0` is how a person says "no limit", and the gate has to prove the CLI reads
  # it that way: the alternative spelling of nothing is a listener that stops
  # answering in the middle of this script.
  AFUI_CONFIG_DIR="$config_root" "$binary" session serve --listen 127.0.0.1:0 \
    --idle-minutes 0 "${extra_args[@]}" >"$log" 2>&1 &
  serve_pid=$!
  # shellcheck disable=SC2064
  trap "kill $serve_pid 2>/dev/null || true; rm -rf '$TEST_TMP'" EXIT

  for _ in $(seq 1 50); do
    [ -s "$log" ] && break
    sleep 0.1
  done
  head -n 1 "$log" >"$TEST_TMP/serving.json"
  url="$(afdata value "$TEST_TMP/serving.json" progress.serve_url --default "")"
  if [ -z "$url" ]; then
    afdata_error serve_not_started \
      "afui session serve did not report a URL" \
      "inspect $log" || :
    return 1
  fi
  # `http://<host>/<credential>/` split into the name the request claims and the
  # path it asks for.
  host="${url#http://}"
  page="/${host#*/}"
  host="${host%%/*}"
  wire="127.0.0.1:${host##*:}"

  for path in "$page" "${page}sessions"; do
    status="$(curl -sS -o /dev/null -w '%{http_code}' -H "Host: $host" "http://$wire$path")"
    [ "$status" = "200" ] || {
      afdata_error serve_page_unavailable "$path answered $status, expected 200" || :
      return 1
    }
  done

  # Anyone without the credential, and anyone guessing a proxy path.
  for path in "/" "/sessions" "/0000000000000000000000000000000000000000000000000000000000000000/" \
              "/s/00112233445566778899aabbccddeeff/"; do
    status="$(curl -sS -o /dev/null -w '%{http_code}' -H "Host: $host" "http://$wire$path")"
    [ "$status" = "404" ] || {
      afdata_error serve_unauthenticated_allowed \
        "$path answered $status; an unauthenticated request must be refused" || :
      return 1
    }
  done

  if [ -n "$session_origin_host" ]; then
    # The page answers under one name and no other, and a session's name opens
    # only the mount it belongs to — of which there are none here.
    local -a refused=(
      "127.0.0.1:${host##*:}|$page"
      "s-$(printf 'a%.0s' $(seq 1 52)).$host|/"
      "s-nope.$host|/"
    )
    local pair claimed
    for pair in "${refused[@]}"; do
      claimed="${pair%%|*}"
      path="${pair##*|}"
      status="$(curl -sS -o /dev/null -w '%{http_code}' -H "Host: $claimed" "http://$wire$path")"
      [ "$status" = "404" ] || {
        afdata_error serve_foreign_name_allowed \
          "$claimed$path answered $status; only the page's own name and a live session's may" || :
        return 1
      }
    done
  fi

  kill "$serve_pid" 2>/dev/null || true
  wait "$serve_pid" 2>/dev/null || true
  # shellcheck disable=SC2064
  trap "rm -rf '$TEST_TMP'" EXIT
}

# The release build, checked the way a release ships it.
#
# `serve` is off by default on purpose (library consumers must not pay for a
# reverse proxy), which means a release build that forgets `--features serve`
# still produces a working-looking afui — one whose single remote-access verb
# answers `feature_unavailable`. Nothing else in this gate would notice, and by
# the time the Release workflow noticed it would be after `cargo publish`.
run_release_smoke() {
  afdata_run --quiet cargo build --release --features serve --bin afui
  afdata_run --quiet ./scripts/release-smoke.sh target/release
}

case "$MODE" in
  static)        run_static ;;
  unit)          run_unit ;;
  smoke)         run_smoke ;;
  release-smoke) run_release_smoke ;;
  # `all` is the release gate, so everything that can fail a release has to be
  # able to fail here first — including the release smoke, which otherwise runs
  # only inside the Release workflow, i.e. after the crate is published.
  all)           run_static; run_unit; run_smoke; run_release_smoke ;;
  *)
    afdata_error cli_error "unsupported test mode '$MODE'" \
      "try: $0 static|unit|smoke|release-smoke|all" || :
    exit 2
    ;;
esac

afdata_result "agent-first-ui tests complete [$MODE]"
