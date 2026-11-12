#!/usr/bin/env bash
# Item 8: modules linked into the helper binary and a heuristic license
# classification from each module's LICENSE/COPYING file. A real third-party
# notice still needs a human review; this only sizes the job.
set -euo pipefail
cd "$(dirname "$0")/../helper"
export GOTOOLCHAIN=${GOTOOLCHAIN:-go1.27.1}
GOOS=${GOOS:-linux}
go list -deps -f '{{with .Module}}{{.Path}} {{.Version}} {{.Dir}}{{end}}' ./cmd/airhand-p2p-spike |
  sort -u | grep -v '^example.com/' |
  while read -r path ver dir; do
    lic=$(ls "$dir" 2>/dev/null | grep -iE '^(licen[cs]e|copying)' | head -1 || true)
    kind=unknown
    if [ -n "$lic" ]; then
      t=$(head -c 6000 "$dir/$lic" | tr -s '[:space:]' ' ')
      case "$t" in
        *"Apache License"*"Version 2.0"*) kind=Apache-2.0 ;;
        *"Redistribution and use in source and binary forms"*"Neither the name"*) kind=BSD-3-Clause ;;
        *"Redistribution and use in source and binary forms"*) kind=BSD-2-Clause ;;
        *"Permission is hereby granted, free of charge"*) kind=MIT ;;
        *"Mozilla Public License"*) kind=MPL-2.0 ;;
        *"Permission to use, copy, modify, and"*"distribute this software"*) kind=ISC ;;
        *"GNU GENERAL PUBLIC"*|*"GNU LESSER"*) kind=GPL/LGPL ;;
      esac
    fi
    echo "$path $ver $kind"
  done
