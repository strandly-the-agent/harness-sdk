#!/bin/bash
# Secret scan over the staged diff (section 9.5). Prints only line numbers.
# Lines carrying this regex literally (this file, PROMPT.md section 9.5) are not secrets.
hits=$(git diff --cached | grep -vF 'AKIA[0-9A-Z]{16}' | grep -nE 'sk-ant-|sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|aws_secret_access_key|x-api-key: [A-Za-z0-9]' | cut -d: -f1 | tr '\n' ' ')
for v in ANTHROPIC_API_KEY OPENAI_API_KEY AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN; do
  val="${!v}"; [ -n "$val" ] && git diff --cached | grep -qF -- "$val" && hits="$hits $v-value"
done
[ -z "$hits" ] && echo "scan: clean" || { echo "scan: FOUND at diff lines: $hits"; exit 1; }
