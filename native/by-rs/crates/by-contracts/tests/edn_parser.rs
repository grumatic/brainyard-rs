use by_contracts::{parse_map, parse_value, EdnValue};

#[test]
fn parses_known_session_meta_shape() {
    let meta = parse_map(
        r#"{:id "session-1"
            :label "Planning spike"
            :last-active #inst "2026-06-01T00:00:00.000-00:00"
            :tokens 42
            :working-dir nil}"#,
    )
    .expect("session meta should parse");

    assert_eq!(meta.string("id"), Some("session-1"));
    assert_eq!(meta.string("label"), Some("Planning spike"));
    assert_eq!(
        meta.instant("last-active"),
        Some("2026-06-01T00:00:00.000-00:00")
    );
    assert_eq!(meta.i64("tokens"), Some(42));
    assert!(meta.is_nil("working-dir"));
}

#[test]
fn rejects_non_map_roots() {
    let err = parse_map(r#"["not" "a" "map"]"#).expect_err("root vectors are unsupported here");
    assert!(err.to_string().contains("map"));
}

#[test]
fn parses_general_edn_roots_for_snapshot_files() {
    let value =
        parse_value(r#"[:pending {:id "dlg-1"} 0.0]"#).expect("snapshot roots are not always maps");

    assert_eq!(
        value,
        EdnValue::Vector(vec![
            EdnValue::Keyword("pending".to_string()),
            EdnValue::Map(parse_map(r#"{:id "dlg-1"}"#).unwrap()),
            EdnValue::Float(0.0),
        ])
    );
}

#[test]
fn parses_common_clojure_snapshot_forms() {
    let value = parse_value(
        r#"{:id #uuid "123e4567-e89b-12d3-a456-426614174000"
             :modes #{:ask :run}
             :queued (:tool "call")
             :kept #_ "discard me" "value"}"#,
    )
    .expect("snapshot parser should accept common Clojure EDN forms");

    assert_eq!(
        value,
        EdnValue::Map(
            parse_map(
                r#"{:id #uuid "123e4567-e89b-12d3-a456-426614174000"
                :modes #{:ask :run}
                :queued (:tool "call")
                :kept "value"}"#
            )
            .unwrap()
        )
    );
}

#[test]
fn parses_uuid_set_list_and_reader_discard_roots() {
    assert_eq!(
        parse_value(r#"#uuid "123e4567-e89b-12d3-a456-426614174000""#).unwrap(),
        EdnValue::Uuid("123e4567-e89b-12d3-a456-426614174000".to_string())
    );
    assert_eq!(
        parse_value(r#"#{:alpha "beta" 3}"#).unwrap(),
        EdnValue::Set(vec![
            EdnValue::Keyword("alpha".to_string()),
            EdnValue::String("beta".to_string()),
            EdnValue::Integer(3),
        ])
    );
    assert_eq!(
        parse_value(r#"(:call {:id "1"})"#).unwrap(),
        EdnValue::List(vec![
            EdnValue::Keyword("call".to_string()),
            EdnValue::Map(parse_map(r#"{:id "1"}"#).unwrap()),
        ])
    );
    assert_eq!(
        parse_value(r#"#_ {:debug true} [:kept]"#).unwrap(),
        EdnValue::Vector(vec![EdnValue::Keyword("kept".to_string())])
    );
}

#[test]
fn parses_mulog_flake_reader_tag_as_the_underlying_value() {
    assert_eq!(
        parse_value(r#"#mulog/flake "01HWTESTTRACE""#).unwrap(),
        EdnValue::String("01HWTESTTRACE".to_string())
    );
}

#[test]
fn parse_map_still_rejects_vector_roots() {
    let err = parse_map(r#"[:pending]"#).expect_err("map callers should keep map-only contract");

    assert!(err.to_string().contains("map"));
}
