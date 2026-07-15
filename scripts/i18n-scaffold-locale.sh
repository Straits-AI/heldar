#!/usr/bin/env bash
# Scaffold a new docs locale for the Docusaurus site: generate the i18n/<locale> translation trees
# (theme JSON stubs) and mirror the English docs into it as the translation starting point.
#
# Usage:  scripts/i18n-scaffold-locale.sh <locale>      e.g. fr, de, ja, pt-BR, zh-Hant
#
# After running:
#   1. Add '<locale>' to i18n.locales + i18n.localeConfigs (label + htmlLang) in website/docusaurus.config.ts
#   2. Add README.<locale>.md (translate README.md; maintainers: also list it in the release allowlist)
#   3. Translate the generated i18n/<locale>/**/*.{json,md} — see website/i18n/TRANSLATING.md
#   4. cd website && npm run build
# Untranslated pages fall back to English automatically, so partial progress is always shippable.
set -euo pipefail

LOCALE="${1:?usage: scripts/i18n-scaffold-locale.sh <locale>   (e.g. fr, ja, pt-BR, zh-Hant)}"
cd "$(dirname "$0")/../website"

echo "==> generating theme translation stubs for '$LOCALE'"
npm run write-translations -- --locale "$LOCALE"

DEST="i18n/$LOCALE/docusaurus-plugin-content-docs/current"
echo "==> mirroring docs/ into $DEST"
mkdir -p "$DEST"
cp -R docs/. "$DEST/"

COUNT="$(find "$DEST" -name '*.md' | wc -l | tr -d ' ')"
echo
echo "Scaffolded i18n/$LOCALE: theme JSON (code.json, navbar.json, footer.json, current.json) + $COUNT doc files."
echo "Next: add '$LOCALE' to i18n.locales + localeConfigs in docusaurus.config.ts, then translate"
echo "      i18n/$LOCALE/** (see website/i18n/TRANSLATING.md). Untranslated pages fall back to English."
