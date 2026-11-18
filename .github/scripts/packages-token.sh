#!/bin/bash
# CI: which token reads tribalfs' Maven packages (SESL9, oneui-design) on GitHub Packages, for the OneOS app's
# Gradle build (app/app-android/settings.gradle.kts takes GH_PACKAGES_USER and GH_PACKAGES_TOKEN). The fork's own
# .github/workflows/oneios-apk.yml does the same.
#   RUN_TOKEN=<this run's GITHUB_TOKEN> SECRET_TOKEN=<secret GH_PACKAGES_TOKEN, may be empty>
#   SECRET_USER=<secret GH_PACKAGES_USER, may be empty> .github/scripts/packages-token.sh
# First the run's own GITHUB_TOKEN (the job has `permissions: packages: read`): GitHub Packages' Maven registry
# wants a login even for public packages. If GitHub refuses it, the refusal is printed and the optional repository
# secret GH_PACKAGES_TOKEN (a token with read:packages) is tried. Writes GH_PACKAGES_USER, GH_PACKAGES_TOKEN and
# PACKAGES_TOKEN_USED to $GITHUB_ENV; a token is never printed. Exit 1 when neither reads the probe file.
set -euo pipefail
PROBE_POM=https://maven.pkg.github.com/tribalfs/*/io/github/tribalfs/oneui-design/0.9.13+oneui8/oneui-design-0.9.13+oneui8.pom
body=$(mktemp)
trap 'rm -f "$body"' EXIT
probe() { # <user> <token>: prints the HTTP status; the answer's body goes to $body
  curl -sS -g -L -o "$body" -w '%{http_code}' -u "$1:$2" "$PROBE_POM" || echo 000
}
use() { # <user> <token> <what>
  echo "::add-mask::$2"
  { echo "GH_PACKAGES_USER=$1"; echo "GH_PACKAGES_TOKEN=$2"; echo "PACKAGES_TOKEN_USED=$3"; } >> "$GITHUB_ENV"
}
code=$(probe "$GITHUB_ACTOR" "${RUN_TOKEN:?no RUN_TOKEN}")
if [ "$code" = 200 ]; then
  echo "GITHUB_TOKEN of $GITHUB_REPOSITORY reads tribalfs' packages (HTTP 200 for oneui-design's pom)."
  use "$GITHUB_ACTOR" "$RUN_TOKEN" GITHUB_TOKEN
  exit 0
fi
echo "::warning::GitHub Packages refused GITHUB_TOKEN of $GITHUB_REPOSITORY for tribalfs' packages: HTTP $code"
echo "--- the refusal, as GitHub sent it ---"; head -c 2000 "$body"; echo; echo "--------------------------------------"
if [ -z "${SECRET_TOKEN:-}" ]; then
  echo "::error::GITHUB_TOKEN was refused (HTTP $code) and the repository has no GH_PACKAGES_TOKEN secret. Add one: a token with the read:packages scope (Settings, Secrets and variables, Actions)."
  exit 1
fi
user=${SECRET_USER:-$GITHUB_REPOSITORY_OWNER}
code=$(probe "$user" "$SECRET_TOKEN")
if [ "$code" != 200 ]; then
  echo "::error::The GH_PACKAGES_TOKEN secret was refused too: HTTP $code"
  head -c 2000 "$body"; echo
  exit 1
fi
echo "The GH_PACKAGES_TOKEN secret reads tribalfs' packages (HTTP 200)."
use "$user" "$SECRET_TOKEN" "GH_PACKAGES_TOKEN secret"
