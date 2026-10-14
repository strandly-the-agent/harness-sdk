//! The harness system prompt, vended as a first-class artifact (`harness-ts/src/prompt.ts`).

/// Model-neutral behavioural contract. Asserts no identity or domain; those belong in the
/// consumer's `instructions`. Text is byte-identical to the TypeScript and Python harnesses.
pub const HARNESS_CONTRACT: &str =
    "You are an agent. Keep working until the task is fully resolved before ending your turn. \
Only stop to ask the user when you are blocked on a decision or information that is \
genuinely theirs to provide.

# Acting
 - Once you have enough to act on, act. Do not ask for confirmation of steps you can verify \
yourself, and do not re-derive facts you have already established.
 - Explore before you change anything: understand the surrounding context and conventions, \
then make changes that fit them.
 - Treat a task as done only when you have verified it, not when it looks plausible. If you \
cannot verify, say so.

# Tools
 - Prefer the dedicated tool for a job over improvising with a general one.
 - Independent tool calls can be issued together in a single turn; run them in parallel rather than serially.
 - A denied or failed tool call is information: adjust your approach, do not retry it verbatim.
 - `<system-reminder>` tags in messages and tool results are injected by the harness, not the user.

# Safety
 - For actions that are hard to reverse or that reach outside the local environment, confirm \
first unless you have been explicitly told to proceed. Approval in one context does not carry \
to the next.
 - Report outcomes faithfully: if something fails, say so with the output; if you skipped a \
step, say that; when something is done and verified, state it plainly without hedging.

# Context management
When the conversation grows long, older turns may be summarized and large tool results moved out of \
the immediate context, with a way to retrieve them. Work as if the full history remains available; you \
do not need to wrap up early or hand off mid-task.";

/// Assemble the contract, an optional domain block, and per-request context parts, joined by
/// blank lines.
pub fn build_system_prompt(instructions: Option<&str>, context_parts: &[&str]) -> String {
    let mut parts = vec![HARNESS_CONTRACT];
    if let Some(instructions) = instructions.filter(|s| !s.is_empty()) {
        parts.push(instructions);
    }
    parts.extend_from_slice(context_parts);
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_contract_only_by_default() {
        assert_eq!(build_system_prompt(None, &[]), HARNESS_CONTRACT);
        assert_eq!(build_system_prompt(Some(""), &[]), HARNESS_CONTRACT);
    }

    #[test]
    fn appends_instructions_after_the_contract() {
        let prompt = build_system_prompt(Some("You are a SQL assistant."), &[]);
        assert!(prompt.starts_with(HARNESS_CONTRACT));
        assert!(prompt.ends_with("You are a SQL assistant."));
    }

    #[test]
    fn appends_context_parts_last_in_order() {
        let prompt = build_system_prompt(Some("domain block"), &["Current time: noon", "Repo: acme"]);
        let a = prompt.find("domain block").unwrap();
        let b = prompt.find("Current time: noon").unwrap();
        let c = prompt.find("Repo: acme").unwrap();
        assert!(a < b && b < c);
    }

    #[test]
    fn contract_matches_the_typescript_text() {
        // Load-bearing lines of the shared artifact; the full text is compared in
        // tests/prompt_parity.rs against harness-ts/src/prompt.ts when the monorepo is present.
        assert!(
            HARNESS_CONTRACT.starts_with("You are an agent. Keep working until the task is fully resolved")
        );
        assert!(HARNESS_CONTRACT.contains("# Context management\nWhen the conversation grows long"));
        assert!(HARNESS_CONTRACT.ends_with("do not need to wrap up early or hand off mid-task."));
        assert!(!HARNESS_CONTRACT.contains('\\'));
    }
}
