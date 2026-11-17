# Track h12vb dispositions (Phase 12 verification pass, SDK side)

| finding | disposition | commit |
|---|---|---|
| correctness-b-3 | Fixed. `strip_unknown_properties` walks `prefixItems` by index and uses `items` for the elements after the prefix, so a typed tool input and a structured output with a tuple field drop an undeclared key of a tuple item, as zod does inside `z.tuple`. Tests: `tools::function_tool::tests::typed::an_undeclared_key_inside_a_tuple_item_is_dropped_and_the_tool_runs`, `tools::function_tool::tests::input_validator::undeclared_keys_are_dropped_from_tuple_items_by_position`, `tools::structured_output::tests::an_undeclared_key_inside_a_tuple_item_is_dropped`. Replacement row for `strands-rs/docs/DIVERGENCES.md` staged in `divergences.md`; P-1220. | 83a5fa7ed |
