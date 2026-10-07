#!/usr/bin/env bash
# Bring a throwaway local Buzz relay up or down for the end-to-end tests
# (tasks 2.9 and 7.1, design section 16.4). Never points at a live relay.
#
#   scripts/e2e-relay.sh up [--build]   start Postgres, Redis and buzz-relay; print the URL
#   scripts/e2e-relay.sh down           remove the relay, the containers and their volumes
#   scripts/e2e-relay.sh status         show the containers and the relay's readiness
#   scripts/e2e-relay.sh logs           print the relay's log
#
# Setup, as task 2.9 lists it:
#   1. Buzz is checked out at the pinned rev (sparse: only the files used here).
#   2. Postgres 17 and Redis 7 come from Buzz's docker-compose.harness.yml under a dedicated
#      Compose project. That file has no fixed container or volume names, so `down -v` can
#      never touch a developer's own `buzz-*` dev stack.
#   3. buzz-relay runs from Buzz's published image for the pinned rev (or, with --build, an
#      image built from that checkout), with `.env.example`, after `buzz-admin migrate`, the
#      way `just relay` runs it.
#   4. Relay auth admits any NIP-42 identity, so the per-run test identities need no
#      registration: membership and the pubkey allowlist are off, and NIP-OA owner-attested
#      auth is on. `up` prints these settings.
#
# Environment (all optional):
#   BUZZ_E2E_RELAY_PORT   host port of the relay, bound to 127.0.0.1 only (default 3000)
#   BUZZ_E2E_PROJECT      Compose project and container prefix (default buzz-router-e2e)
#   BUZZ_E2E_BUZZ_DIR     Buzz checkout to use or create (default: a cache dir per rev)
#   BUZZ_E2E_RELAY_IMAGE  relay image (default ghcr.io/block/buzz:sha-<short rev>)
set -euo pipefail

# The Buzz commit this repository pins (brief section 2, Cargo.toml).
BUZZ_REV="f0eb5575ffc9d5f57af4ed3f574529d997c83a0d"
BUZZ_REPO="https://github.com/block/buzz"

PORT="${BUZZ_E2E_RELAY_PORT:-3000}"
PROJECT="${BUZZ_E2E_PROJECT:-buzz-router-e2e}"
BUZZ_DIR="${BUZZ_E2E_BUZZ_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/buzz-router/buzz-${BUZZ_REV}}"
PUBLISHED_IMAGE="ghcr.io/block/buzz:sha-${BUZZ_REV:0:7}"
BUILT_IMAGE="${PROJECT}-relay:${BUZZ_REV:0:12}"
RELAY_CONTAINER="${PROJECT}-relay"
NETWORK="${PROJECT}_default"
COMPOSE_FILE="docker-compose.harness.yml"
RELAY_URL="ws://127.0.0.1:${PORT}"
# The community host the relay resolves requests to: RELAY_URL's authority.
COMMUNITY_HOST="127.0.0.1:${PORT}"

log() { printf '[e2e-relay] %s\n' "$*" >&2; }
die() {
  log "error: $*"
  exit 1
}

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

require() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required but not installed"
  done
}

compose() {
  docker compose -p "$PROJECT" -f "$BUZZ_DIR/$COMPOSE_FILE" "$@"
}

# A value from Buzz's .env.example, such as PGUSER.
env_example() {
  local value
  value="$(sed -n "s/^$1=//p" "$BUZZ_DIR/.env.example" | head -n 1)"
  [[ -n "$value" ]] || die "$1 is not set in $BUZZ_DIR/.env.example"
  printf '%s' "$value"
}

# Makes $BUZZ_DIR a checkout of BUZZ_REV. Sparse unless a full tree is needed to build.
checkout_buzz() {
  local full="$1"
  if [[ ! -d "$BUZZ_DIR/.git" ]]; then
    log "fetching Buzz $BUZZ_REV into $BUZZ_DIR"
    mkdir -p "$BUZZ_DIR"
    git -C "$BUZZ_DIR" init -q
    git -C "$BUZZ_DIR" remote add origin "$BUZZ_REPO"
  fi
  if [[ "$full" == "full" ]]; then
    git -C "$BUZZ_DIR" sparse-checkout disable
  elif [[ "$(git -C "$BUZZ_DIR" rev-parse HEAD 2>/dev/null || true)" != "$BUZZ_REV" ]]; then
    git -C "$BUZZ_DIR" sparse-checkout set --no-cone "/$COMPOSE_FILE" /.env.example
  fi
  if [[ "$(git -C "$BUZZ_DIR" rev-parse HEAD 2>/dev/null || true)" != "$BUZZ_REV" ]]; then
    git -C "$BUZZ_DIR" fetch -q --depth 1 --filter=blob:none origin "$BUZZ_REV"
  fi
  git -C "$BUZZ_DIR" checkout -q --detach "$BUZZ_REV"
  [[ "$(git -C "$BUZZ_DIR" rev-parse HEAD)" == "$BUZZ_REV" ]] ||
    die "$BUZZ_DIR is not at $BUZZ_REV"
  [[ -f "$BUZZ_DIR/$COMPOSE_FILE" && -f "$BUZZ_DIR/.env.example" ]] ||
    die "$BUZZ_DIR lacks $COMPOSE_FILE or .env.example"
}

# Prints the relay image to run, pulling or building it.
relay_image() {
  local build="$1" image revision
  if [[ "$build" == "build" ]]; then
    image="$BUILT_IMAGE"
    log "building $image from $BUZZ_DIR (this takes a while)"
    docker build -q --target runtime -t "$image" "$BUZZ_DIR" >/dev/null
  else
    image="${BUZZ_E2E_RELAY_IMAGE:-$PUBLISHED_IMAGE}"
    log "pulling $image"
    docker pull -q "$image" >/dev/null
    revision="$(docker image inspect --format \
      '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$image")"
    if [[ -n "$revision" && "$revision" != "$BUZZ_REV" ]]; then
      die "$image was built from $revision, not the pinned $BUZZ_REV"
    fi
    [[ -n "$revision" ]] || log "warning: $image has no revision label to check"
  fi
  printf '%s' "$image"
}

wait_ready() {
  local code
  for _ in $(seq 1 90); do
    if [[ "$(docker inspect -f '{{.State.Running}}' "$RELAY_CONTAINER" 2>/dev/null)" != "true" ]]; then
      docker logs "$RELAY_CONTAINER" >&2 2>&1 || true
      die "the relay container exited"
    fi
    code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/_readiness" || true)"
    [[ "$code" == "200" ]] && return 0
    sleep 1
  done
  docker logs --tail 50 "$RELAY_CONTAINER" >&2 2>&1 || true
  die "the relay was not ready within 90 s"
}

cmd_up() {
  local build="pull" image pg_user pg_password pg_db database_url
  case "${1:-}" in
    "") ;;
    --build) build="build" ;;
    *) usage ;;
  esac
  require docker git curl od
  [[ "$PORT" =~ ^[0-9]+$ ]] || die "BUZZ_E2E_RELAY_PORT must be a port number"
  if docker inspect "$RELAY_CONTAINER" >/dev/null 2>&1; then
    die "$RELAY_CONTAINER already exists; run '$0 down' first"
  fi

  checkout_buzz "$([[ "$build" == "build" ]] && echo full || echo sparse)"

  # MinIO is deliberately not started: its pinned image lives on quay.io, which
  # refuses anonymous pulls (401), and nothing in tasks 2.9/7.x touches S3-backed
  # features (media, git object storage). The relay's git object-store
  # conformance probe is therefore disabled with its official kill-switch
  # (BUZZ_GIT_CONFORMANCE_PROBE=false, the same flag Buzz's own
  # boot_lifecycle tests use); everything else boots exactly as `just relay`.
  log "starting postgres and redis (project $PROJECT)"
  compose up -d --wait postgres redis

  image="$(relay_image "$build")"
  pg_user="$(env_example PGUSER)"
  pg_password="$(env_example PGPASSWORD)"
  pg_db="$(env_example PGDATABASE)"
  database_url="postgres://${pg_user}:${pg_password}@postgres:5432/${pg_db}"

  log "running database migrations"
  docker run --rm --network "$NETWORK" -e DATABASE_URL="$database_url" \
    --entrypoint buzz-admin "$image" migrate >/dev/null

  log "seeding community host $COMMUNITY_HOST"
  compose exec -T postgres psql -q -U "$pg_user" -d "$pg_db" -v ON_ERROR_STOP=1 -c \
    "INSERT INTO communities (host) VALUES ('${COMMUNITY_HOST}') ON CONFLICT (lower(host)) DO NOTHING;"

  # A fresh relay signing key per run. It stays in this process's environment and the
  # container's; it is never written to a file.
  BUZZ_RELAY_PRIVATE_KEY="$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')"
  export BUZZ_RELAY_PRIVATE_KEY

  log "starting buzz-relay on $RELAY_URL"
  docker run -d --name "$RELAY_CONTAINER" --network "$NETWORK" \
    --label "com.buzz-router.e2e=$PROJECT" \
    -p "127.0.0.1:${PORT}:3000" \
    --env-file "$BUZZ_DIR/.env.example" \
    -e DATABASE_URL="$database_url" \
    -e REDIS_URL=redis://redis:6379 \
    -e BUZZ_GIT_CONFORMANCE_PROBE=false \
    -e RELAY_URL="$RELAY_URL" \
    -e BUZZ_BIND_ADDR=0.0.0.0:3000 \
    -e BUZZ_RELAY_PRIVATE_KEY \
    -e BUZZ_AUTO_MIGRATE=false \
    -e BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
    -e BUZZ_PUBKEY_ALLOWLIST=false \
    -e BUZZ_ALLOW_NIP_OA_AUTH=true \
    -e BUZZ_RECONCILE_CHANNELS=true \
    -e BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN=100000 \
    -e BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN=100000 \
    -e BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC=10000 \
    -e RUST_LOG=info \
    "$image" >/dev/null
  unset BUZZ_RELAY_PRIVATE_KEY

  wait_ready
  log "ready. Relay settings: image=$image membership=off allowlist=off nip-oa-auth=on"
  log "  community=$COMMUNITY_HOST, Buzz .env.example plus the overrides in $0"
  printf 'BUZZ_E2E_RELAY_URL=%s\n' "$RELAY_URL"
}

cmd_down() {
  require docker
  log "removing $RELAY_CONTAINER"
  docker rm -f "$RELAY_CONTAINER" >/dev/null 2>&1 || true
  log "removing project $PROJECT and its volumes"
  if [[ -f "$BUZZ_DIR/$COMPOSE_FILE" ]]; then
    compose down -v --remove-orphans
  else
    docker compose -p "$PROJECT" down -v --remove-orphans
  fi
}

cmd_status() {
  require docker curl
  docker ps -a --filter "label=com.docker.compose.project=$PROJECT" \
    --format '{{.Names}}\t{{.Status}}'
  docker ps -a --filter "name=^${RELAY_CONTAINER}$" --format '{{.Names}}\t{{.Status}}'
  printf 'readiness: %s\n' \
    "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/_readiness" || true)"
}

case "${1:-}" in
  up) shift; cmd_up "$@" ;;
  down) cmd_down ;;
  status) cmd_status ;;
  logs) docker logs "$RELAY_CONTAINER" ;;
  *) usage ;;
esac
