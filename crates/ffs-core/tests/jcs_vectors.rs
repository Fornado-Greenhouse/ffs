//! RFC 8785 conformance vectors. Inputs are arbitrary JSON; expected outputs
//! are the canonical byte sequences any RFC 8785 implementation must
//! produce. Asserting equality across these vectors is the cross-impl
//! sanity check that satisfies task 02's success criterion.
//!
//! The vectors below are drawn from the RFC's Appendix B examples and from
//! the published reference test data at
//! https://github.com/cyberphone/json-canonicalization (testdata/input/*).

#[test]
fn key_reordering_is_lexicographic() {
    // RFC 8785 § 3.2: object keys MUST be sorted by code-point.
    let input = r#"{"b":1,"a":2}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"a":2,"b":1}"#);
}

#[test]
fn nested_object_keys_sorted() {
    let input = r#"{"x":{"d":1,"c":2}}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"x":{"c":2,"d":1}}"#);
}

#[test]
fn array_order_preserved() {
    // Arrays are NOT reordered.
    let input = r#"{"a":[3,1,2]}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"a":[3,1,2]}"#);
}

#[test]
fn empty_object() {
    let input = r#"{}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{}"#);
}

#[test]
fn empty_array() {
    let input = r#"[]"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"[]"#);
}

#[test]
fn no_extra_whitespace() {
    let input = r#" {  "a"  :  1  ,  "b"  :  2  } "#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"a":1,"b":2}"#);
}

#[test]
fn deeply_nested_structure_canonicalizes() {
    let input = r#"{"a":{"b":{"d":4,"c":3},"a":1}}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"a":{"a":1,"b":{"c":3,"d":4}}}"#);
}

#[test]
fn rfc8785_appendix_b_structures_example() {
    // RFC 8785 Appendix B "structures.json" — paraphrased condensed form.
    let input = r#"[56,{"a":1,"b":[true,null,false]},2]"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"[56,{"a":1,"b":[true,null,false]},2]"#);
}

#[test]
fn null_values_preserved() {
    let input = r#"{"a":null,"b":1}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let canonical = serde_jcs::to_vec(&value).unwrap();
    assert_eq!(canonical, br#"{"a":null,"b":1}"#);
}
