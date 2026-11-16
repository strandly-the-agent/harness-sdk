Replacement for the `strands-rs/docs/DIVERGENCES.md` row "A stage stream that ends without a result" (parity-b7):

| A stage stream that ends without a result | A generator always returns (possibly `undefined`) | `the <stage> middleware chain ended without a result`: it fails the invocation for `agentStream`, `invokeModel` and, under the sequential executor, `executeTool`; under the concurrent executor it is that tool's error result and the turn goes on (`agent::run::tests::middleware::chain_without_result`) | Streams have no return value |
