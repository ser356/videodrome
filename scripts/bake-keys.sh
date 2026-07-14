#!/usr/bin/env zsh
# Reemplaza los placeholders __LB_APP_*__ en src/config.rs y src/subtitles.rs
# con los valores reales de tu ~/.config/letterboxd-cli/.env
#
# Uso: ./scripts/bake-keys.sh
#
# Deja el árbol listo para committear. Después haz:
#   git commit -m "chore: bake app credentials into source"
#   git push
#   git tag v0.1.0 -f && git push --force origin v0.1.0

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

set -a
for f in "$HOME/.config/letterboxd-cli/.env" "$ROOT/.env"; do
  [[ -f "$f" ]] && source "$f"
done
set +a

check_var() {
  local name="$1"
  if [[ -z "${(P)name:-}" ]]; then
    echo "❌ Variable $name no está definida en ~/.config/letterboxd-cli/.env ni en el .env del repo" >&2
    exit 1
  fi
}

check_var LETTERBOXD_CLIENT_ID
check_var LETTERBOXD_CLIENT_SECRET
check_var TMDB_BEARER_TOKEN
check_var OPENSUBTITLES_API_KEY

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$ROOT/src/config.rs"
SUBS="$ROOT/src/subtitles.rs"

for f in "$CONFIG" "$SUBS"; do
  if ! grep -q "__LB_APP_" "$f"; then
    echo "⚠️  $f no contiene placeholders — ¿ya estaba baked?" >&2
    exit 1
  fi
done

sed -i.bak "s|__LB_APP_CLIENT_ID__|${LETTERBOXD_CLIENT_ID}|" "$CONFIG"
sed -i.bak "s|__LB_APP_CLIENT_SECRET__|${LETTERBOXD_CLIENT_SECRET}|" "$CONFIG"
sed -i.bak "s|__LB_APP_TMDB_BEARER__|${TMDB_BEARER_TOKEN}|" "$CONFIG"
sed -i.bak "s|__LB_APP_OS_API_KEY__|${OPENSUBTITLES_API_KEY}|" "$SUBS"

rm -f "$CONFIG.bak" "$SUBS.bak"

echo "✅ Claves baked en src/config.rs y src/subtitles.rs"
echo
echo "Verifica:"
echo "  cargo build --release"
echo "  git diff src/config.rs src/subtitles.rs"
echo
echo "Si todo bien, commit + push."
