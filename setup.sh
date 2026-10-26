#!/usr/bin/env bash
# Sets up Chat and generates every password and key it needs. Run it again after
# pulling a new release; it keeps the passwords and keys it made.
#
#   ./setup.sh docker      Chat and PostgreSQL with Docker or Podman Compose
#   ./setup.sh kubernetes  Chat with workspaces in one namespace
#
# It asks for anything missing; pass it instead with:
#   --url URL         the address people open Chat at
#   --gateway URL     the Obleth gateway, as Chat reaches it
#   --namespace NAME  Kubernetes only (default obleth-chat)
#   --database URL    Kubernetes only: an existing PostgreSQL 17 with pgvector.
#                     Without it, PostgreSQL runs in the namespace.
#   --storage-class NAME
#                     Kubernetes only: for PostgreSQL's and workspaces' volumes,
#                     instead of the cluster's default StorageClass
#   --yes             use the defaults instead of asking
set -euo pipefail
cd "$(dirname "$0")"

usage() { awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit "${1:-0}"; }
die() { printf 'setup: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is needed $2"; }

mode=${1:-}
[ $# -gt 0 ] && shift
url='' gateway='' namespace='' database='' storage_class='' assume_yes=''
while [ $# -gt 0 ]; do
  case $1 in
    --url) url=${2:-}; shift 2 ;;
    --gateway) gateway=${2:-}; shift 2 ;;
    --namespace) namespace=${2:-}; shift 2 ;;
    --database) database=${2:-}; shift 2 ;;
    --storage-class) storage_class=${2:-}; shift 2 ;;
    -y | --yes) assume_yes=1; shift ;;
    -h | --help) usage ;;
    *) die "unknown option $1 (see ./setup.sh --help)" ;;
  esac
done

# ask VAR QUESTION [DEFAULT]: keeps VAR if it is set, else asks on a terminal.
ask() {
  local answer
  eval "answer=\${$1}"
  if [ -z "$answer" ] && [ -t 0 ] && [ -z "$assume_yes" ]; then
    read -r -p "$2${3:+ [$3]}: " answer
  fi
  answer=${answer:-${3:-}}
  eval "$1=\$answer"
}
check_url() {
  case $2 in http://?* | https://?*) ;; *) die "$1 must start with http:// or https://" ;; esac
  case $2 in *[[:space:]\"\'\\]*) die "$1 must not contain spaces or quotes" ;; esac
}
hex() { od -An -tx1 -N32 /dev/urandom | tr -d ' \n'; }
# head closes the pipe early, which pipefail would otherwise count as a failure.
password() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24 || true; }
# An RSA key in PKCS#8 PEM, with whichever tool this machine has.
signing_key() {
  if command -v openssl >/dev/null 2>&1; then
    openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$1" 2>/dev/null
  elif command -v ssh-keygen >/dev/null 2>&1; then
    ssh-keygen -q -t rsa -b 3072 -m PKCS8 -N '' -C '' -f "$1" >/dev/null && rm -f "$1.pub"
  else
    die "openssl or ssh-keygen is needed to make Chat's signing key."
  fi
}

# The entry that makes the gateway trust Chat's tokens.
gateway_entry() {
  cat <<EOF
  {
    "issuer": "$1",
    "jwks_url": "$2",
    "audience": "obleth",
    "algorithms": ["RS256"],
    "subject_claim": "sub",
    "identity_claims": ["username"],
    "tenant": "chat-users",
    "jit_provision": true
  }
EOF
}
limits() {
  cat <<EOF
Limits:
  - Workspaces and the browser run only on Kubernetes.$([ "$1" = docker ] && printf ' This install runs without them.')
  - Previews of apps built in a workspace need a wildcard DNS name and certificate,
    so each app gets its own address. Without one, everything else works.
    https://www.obleth.com/docs/run-chat/enable-workspaces
EOF
}

setup_docker() {
  need curl "to check that Chat started."
  local compose
  if docker compose version >/dev/null 2>&1; then compose="docker compose"
  elif command -v docker-compose >/dev/null 2>&1; then compose=docker-compose
  elif podman compose version >/dev/null 2>&1; then compose="podman compose"
  elif command -v podman-compose >/dev/null 2>&1; then compose=podman-compose
  else die "Docker Compose or Podman Compose is needed."
  fi

  local env=.env
  touch "$env"
  chmod 600 "$env"
  env_get() { sed -n "s/^$1=//p" "$env" | tail -n 1; }
  env_put() {
    local rest
    rest=$(grep -v "^$1=" "$env" || true)
    printf '%s\n%s=%s\n' "$rest" "$1" "$2" | sed '/^$/d' >"$env"
  }

  url=${url:-$(env_get OBLETH_CHAT_ORIGIN)}
  gateway=${gateway:-$(env_get OBLETH_CHAT_GATEWAY_URL)}
  ask url "Address people will open Chat at" "http://localhost:8085"
  ask gateway "Obleth gateway URL, as the Chat container reaches it (a gateway on this host is http://host.docker.internal:<port>)"
  url=${url%/}
  check_url --url "$url"
  [ -n "$gateway" ] || die "--gateway is needed: Chat gets its models from the gateway."
  check_url --gateway "$gateway"
  env_put OBLETH_CHAT_ORIGIN "$url"
  env_put OBLETH_CHAT_GATEWAY_URL "$gateway"

  # Passwords are made once: the database keeps the first ones it was given.
  local admin_password new_admin=
  [ -n "$(env_get OBLETH_CHAT_POSTGRES_ADMIN_PASSWORD)" ] || env_put OBLETH_CHAT_POSTGRES_ADMIN_PASSWORD "$(hex)"
  [ -n "$(env_get OBLETH_CHAT_POSTGRES_PASSWORD)" ] || env_put OBLETH_CHAT_POSTGRES_PASSWORD "$(hex)"
  if [ -z "$(env_get OBLETH_CHAT_BOOTSTRAP_PASSWORD)" ]; then
    env_put OBLETH_CHAT_BOOTSTRAP_PASSWORD "$(password)"
    new_admin=1
  fi
  admin_password=$(env_get OBLETH_CHAT_BOOTSTRAP_PASSWORD)

  # The container runs as UID 10001 and reads the key through a bind mount, so the
  # file is readable; its directory is not, for anyone else on this host.
  mkdir -p .local
  chmod 700 .local
  local key=$PWD/.local/gateway_signing_key.pem
  [ -s "$key" ] || signing_key "$key"
  chmod 644 "$key"
  env_put OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE "$key"

  local files=(--env-file "$env" -f deploy/compose.connected.yaml)
  $compose "${files[@]}" pull
  $compose "${files[@]}" up -d

  local port
  port=$(env_get OBLETH_CHAT_PORT)
  port=${port:-8085}
  printf 'Waiting for Chat'
  local i
  for i in $(seq 1 90); do
    if curl -fsS "http://127.0.0.1:$port/readyz" >/dev/null 2>&1; then
      break
    fi
    [ "$i" -lt 90 ] || { echo; die "Chat didn't become ready. See: $compose ${files[*]} logs obleth-chat"; }
    printf .
    sleep 2
  done
  echo

  cat <<EOF

Chat is running on 127.0.0.1:$port.
  Sign in at $url as admin$([ -n "$new_admin" ] && printf ' with %s' "$admin_password" || printf ', with the initial password in .env (OBLETH_CHAT_BOOTSTRAP_PASSWORD)')

Next:
EOF
  case $url in
    http://localhost:"$port" | http://127.0.0.1:"$port") ;;
    *) echo "  - Route $url to 127.0.0.1:$port in your reverse proxy, with response buffering off and long read timeouts: replies stream." ;;
  esac
  cat <<EOF
  - Add this entry to the gateway's issuers (OBLETH_JWT_ISSUERS, or obleth.jwtIssuers in
    Helm) and restart the gateway. jwks_url must be an address where the gateway reaches Chat.
$(gateway_entry "$url" "$url/.well-known/jwks.json")

To restart or update later: $compose ${files[*]} up -d
$(limits docker)
EOF
}

setup_kubernetes() {
  need kubectl "and access to your cluster."
  kubectl get --raw /version >/dev/null 2>&1 || die "kubectl can't reach the cluster."

  ask namespace "Namespace" "obleth-chat"
  ask url "Address people will open Chat at, through your ingress (https://…)"
  ask gateway "Obleth gateway URL inside the cluster (such as http://obleth-gateway.obleth.svc.cluster.local:8080)"
  [[ $namespace =~ ^[a-z0-9]([-a-z0-9]*[a-z0-9])?$ ]] || die "--namespace must be a valid namespace name."
  [ -n "$url" ] || die "--url is needed."
  url=${url%/}
  check_url --url "$url"
  [ -n "$gateway" ] || die "--gateway is needed: Chat gets its models from the gateway."
  check_url --gateway "$gateway"
  # Without a StorageClass, PostgreSQL and workspaces would wait forever for volumes.
  if [ -n "$storage_class" ]; then
    kubectl get storageclass "$storage_class" >/dev/null 2>&1 || die "StorageClass $storage_class doesn't exist."
  elif ! kubectl get storageclass -o jsonpath='{range .items[*]}{.metadata.annotations.storageclass\.kubernetes\.io/is-default-class}{"\n"}{end}' | grep -qx true; then
    die "The cluster has no default StorageClass. Pass --storage-class, one of: $(kubectl get storageclass -o name | sed 's|.*/||' | tr '\n' ' ')"
  fi

  local ns=(-n "$namespace")
  has_secret() { kubectl "${ns[@]}" get secret "$1" >/dev/null 2>&1; }
  secret_value() { kubectl "${ns[@]}" get secret "$1" -o "go-template={{index .data \"$2\"}}" | base64 -d; }
  # deploy/*.yaml for this namespace and these URLs.
  render() {
    awk -v ns="$namespace" -v origin="$url" -v gateway="$gateway" -v storage="$storage_class" '
      /^---$/ { kind = "" }
      /^kind: / { kind = $2 }
      kind == "Namespace" && $0 == "  name: obleth-chat" { print "  name: " ns; next }
      /^ *namespace: obleth-chat$/ { sub(/obleth-chat$/, ns); print; next }
      /^  OBLETH_CHAT_ORIGIN: / { print "  OBLETH_CHAT_ORIGIN: " origin; next }
      /^  OBLETH_CHAT_GATEWAY_URL: / {
        print "  OBLETH_CHAT_GATEWAY_URL: " gateway
        if (storage != "") print "  OBLETH_CHAT_SANDBOX_STORAGE_CLASS: " storage
        next
      }
      /^  OBLETH_CHAT_GATEWAY_ISSUER: / { print "  OBLETH_CHAT_GATEWAY_ISSUER: " origin; next }
      /^        accessModes: \[ReadWriteOnce\]$/ && storage != "" { print; print "        storageClassName: " storage; next }
      { print }' "$1"
  }

  kubectl get namespace "$namespace" >/dev/null 2>&1 || kubectl create namespace "$namespace" --save-config >/dev/null
  # As in deploy/k8s.yaml, before anything starts here.
  kubectl label namespace "$namespace" --overwrite >/dev/null \
    pod-security.kubernetes.io/enforce=restricted pod-security.kubernetes.io/enforce-version=latest

  if ! has_secret obleth-chat-gateway-signing-key; then
    local tmp
    tmp=$(mktemp -d)
    signing_key "$tmp/key.pem"
    kubectl "${ns[@]}" create secret generic obleth-chat-gateway-signing-key \
      --from-file=gateway_signing_key="$tmp/key.pem" >/dev/null
    rm -rf "$tmp"
  fi

  # PostgreSQL in the namespace, unless Chat already has a database or was given one.
  if has_secret obleth-chat-postgres || { ! has_secret obleth-chat-env && [ -z "$database" ]; }; then
    has_secret obleth-chat-postgres || kubectl "${ns[@]}" create secret generic obleth-chat-postgres \
      --from-literal=admin-password="$(hex)" --from-literal=chat-password="$(hex)" >/dev/null
    kubectl "${ns[@]}" create configmap obleth-chat-postgres-init \
      --from-file=10-obleth-chat.sh=deploy/postgres-init.sh --dry-run=client -o yaml | kubectl apply -f - >/dev/null
    render deploy/k8s-postgres.yaml | kubectl apply -f -
    kubectl "${ns[@]}" rollout status statefulset/obleth-chat-postgres --timeout=300s
    database=${database:-postgresql://obleth_chat:$(secret_value obleth-chat-postgres chat-password)@obleth-chat-postgres:5432/obleth_chat}
  fi

  local admin_password=
  if has_secret obleth-chat-env; then
    [ -z "$database" ] || has_secret obleth-chat-postgres ||
      echo "Keeping the database in Secret obleth-chat-env; edit that Secret to change it."
  else
    case $database in
      postgres://?* | postgresql://?*) ;;
      *) die "--database must be a postgresql:// URL" ;;
    esac
    admin_password=$(password)
    kubectl "${ns[@]}" create secret generic obleth-chat-env \
      --from-literal=OBLETH_CHAT_DATABASE_URL="$database" \
      --from-literal=OBLETH_CHAT_BOOTSTRAP_PASSWORD="$admin_password" >/dev/null
  fi

  render deploy/k8s.yaml | kubectl apply -f -
  kubectl "${ns[@]}" rollout status deployment/obleth-chat --timeout=300s

  cat <<EOF

Chat is running in namespace $namespace, with workspaces for administrators.
  Sign in at $url as admin$([ -n "$admin_password" ] && printf ' with %s' "$admin_password" || printf ', with the initial password in Secret obleth-chat-env (OBLETH_CHAT_BOOTSTRAP_PASSWORD)')

Next:
  - Route $url to Service obleth-chat, port 8085, in namespace $namespace through your
    ingress, with response buffering off and long read timeouts: replies stream.
  - Add this entry to the gateway's issuers (obleth.jwtIssuers in Helm, or
    OBLETH_JWT_ISSUERS) and restart the gateway:
$(gateway_entry "$url" "http://obleth-chat.$namespace.svc.cluster.local:8085/.well-known/jwks.json")

$(limits kubernetes)
EOF
}

case $mode in
  docker) setup_docker ;;
  kubernetes | k8s) setup_kubernetes ;;
  -h | --help | help) usage ;;
  *) usage 1 ;;
esac
