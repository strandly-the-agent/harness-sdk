#!/bin/bash
# Scans the added lines of the staged diff; prints only diff line numbers, never matching text.
# One character of each literal is bracketed (or split by quotes) so this file never matches itself.
staged(){ git diff --cached --text --no-textconv --no-ext-diff; }
hits=$( {
  staged | grep -naE '^\+.*(s[k]-ant-|s[k]-[A-Za-z0-9_-]{20,}|A[K]IA[0-9A-Z]{16}|A[S]IA[0-9A-Z]{16})'
  staged | grep -naiE '^\+.*(aws_secret_access_ke[y]|x-api-ke[y]: [A-Za-z0-9])'
} | cut -d: -f1 | sort -nu | tr '\n' ' ')
for v in ANTHROPIC_API_KEY OPENAI_API_KEY AWS_SECRET_ACCESS_""KEY AWS_SESSION_TOKEN; do
  val="${!v}"; [ -n "$val" ] && staged | grep -a '^+' | grep -qaF -- "$val" && hits="$hits $v-value"
done
[ -z "$hits" ] && echo "scan: clean" || { echo "scan: FOUND at diff lines: $hits"; exit 1; }
