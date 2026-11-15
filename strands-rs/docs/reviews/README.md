# Reviews

This folder holds the review record for the Rust SDK (`strands` and `strands-macros`).

- `phase-N.md`: the findings from the review at the end of each implementation phase, one entry per finding with the file and line, the trigger, the observed behaviour and the evidence, followed by its disposition (fixed, with the commit or test that covers it, or not a defect, with the reason).
- `live.md`: results of the live tests against real model providers: test names, pass or fail, model ids, regions, and the exact error text on failure. It never contains credentials, account ids or full response bodies.
