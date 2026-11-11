# Workflows waiting to be moved

These are the Rust CI workflows for `.github/workflows/`. The fork push token has no `workflow` scope, so the branch can't carry changes under `.github/workflows/` (P-16). Apply them with a token that has that scope:

```bash
cp .rust-port/workflows/{strands-rs-pr-and-push,strands-rs-test-lint,harness-rs-pr-and-push,harness-rs-test-lint,rust-security-audit}.yml .github/workflows/
git apply .rust-port/workflows/ci.yml.patch   # or copy ci.yml, which is the pinned ci.yml plus the Rust areas
```
