use vdf::controller::{LayoutDocument, MAX_ACTIONS_PER_INPUT};

#[test]
fn preserves_unknown_fields_and_ordered_duplicates() {
    let doc = LayoutDocument::parse(r#""controller_mappings" { "unknown" "a" "unknown" "b" "group" { "id" "1" } "group" { "id" "2" } }"#).unwrap();
    assert_eq!(LayoutDocument::parse(&doc.write()).unwrap(), doc);
    assert_eq!(doc.mappings().iter().filter(|(key, _)| *key == "unknown").count(), 2);
}

#[test]
fn rejects_action_amplification_through_repeated_preset_references() {
    let binding = r#""binding" "game_action Example jump""#;
    let bindings = binding.repeat(MAX_ACTIONS_PER_INPUT);
    let cfg = format!(
        r#""controller_mappings" {{ "group" {{ "id" "1" "inputs" {{ "button_a" {{ "activators" {{ "Full_Press" {{ "bindings" {{ {bindings} }} }} }} }} }} }} "preset" {{ "name" "Example" "group_source_bindings" {{ "1" "button_diamond active" "1" "button_diamond active" }} }} }}"#
    );
    assert!(LayoutDocument::parse(&cfg).is_err());
}
