#!/bin/sh
set -eu

version=${1:?usage: extract-changelog.sh VERSION [CHANGELOG]}
changelog=${2:-CHANGELOG.md}

awk -v heading="## [$version] - " '
  index($0, heading) == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found { print }
  END { if (!found) exit 1 }
' "$changelog"
