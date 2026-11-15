Staged CI changes for the Rust workspace. The run environment's push token cannot create or update files under `.github/workflows/` (GitHub requires the `workflow` scope), so the workflows live here until they are applied (P-12).

Apply from the repo root:

```bash
cp .rust-port/ci/workflows/{strands-rs-pr-and-push,strands-rs-test-lint,harness-rs-pr-and-push,harness-rs-test-lint,rust-security-audit}.yml .github/workflows/
git apply .rust-port/ci/ci.yml.patch   # or: cp .rust-port/ci/workflows/ci.yml .github/workflows/ci.yml
```

`ci.yml.patch` is the diff of `.github/workflows/ci.yml` against the pin `7296caba7`; `workflows/ci.yml` is the full patched file.
