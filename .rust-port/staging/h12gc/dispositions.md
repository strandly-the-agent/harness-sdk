# h12gc dispositions

| finding | disposition | commit |
|---|---|---|
| api-17 | Fixed. `harness-rs/strands-harness/tests/public_api.rs` (11 cases, offline, crate-root names only): `the_create_harness_future_is_send`, `the_builder_takes_a_model_instance_instructions_tools_and_plugins`, `the_struct_form_takes_the_same_options_with_default_for_the_rest`, `a_builtin_tools_list_enables_exactly_those_tools`, `a_builtin_tools_mapping_edits_the_default_set`, `a_session_config_saves_the_turn_under_its_directory`, `session_skills_and_memory_off_leave_the_agent_without_them`, `interventions_take_a_name_or_a_handler`, `context_manager_takes_a_preset_or_off`, `agent_config_passes_the_other_agent_fields_through`, `agent_config_rejects_the_six_keys_the_harness_owns`. The section 13 snippet is the doctest of `create_harness` (api-6), not a case here. | `57c49b917` |
