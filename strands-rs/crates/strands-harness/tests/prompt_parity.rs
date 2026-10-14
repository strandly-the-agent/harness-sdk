//! Byte-for-byte parity of `HARNESS_CONTRACT` with `harness-ts/src/prompt.ts` when the monorepo
//! checkout is present (skipped otherwise).

use std::path::Path;

#[test]
fn harness_contract_matches_the_typescript_source() {
    let ts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../harness-ts/src/prompt.ts");
    let Ok(source) = std::fs::read_to_string(&ts) else {
        eprintln!("harness-ts not present; skipping");
        return;
    };
    let start =
        source.find("export const HARNESS_CONTRACT = `").unwrap() + "export const HARNESS_CONTRACT = `".len();
    let end = source[start..].find("`\n").unwrap() + start;
    let literal = &source[start..end];
    // Resolve the template-literal escapes the TS source uses: `\<newline>` continuations and `\``.
    let expected = literal.replace("\\\n", "").replace("\\`", "`");
    assert_eq!(strands_harness::HARNESS_CONTRACT, expected);
}
