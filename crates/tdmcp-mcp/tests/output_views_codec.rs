use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::error::Error;
use std::io;
use tdmcp_mcp::output_views::{
    CodecError, CodecHeader, CodecLimits, CompactOutput, EncodedOutput, FullReason, OutputCodec,
    PathSegment, PresentationPlan, Transform, CODEC_NAME, CODEC_VERSION,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCase {
    name: String,
    expect_compact: bool,
    value: Value,
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    Ok(serde_json::from_str(include_str!(
        "fixtures/output_views_adversarial.json"
    ))?)
}

fn compact(value: Value, transforms: Vec<Transform>) -> CompactOutput {
    CompactOutput {
        codec: CodecHeader {
            name: CODEC_NAME.to_owned(),
            version: CODEC_VERSION,
        },
        data: value,
        transforms,
    }
}

fn key(value: &str) -> PathSegment {
    PathSegment::Key(value.to_owned())
}

#[test]
fn sanitized_fixture_round_trips_and_plans_adaptively() -> Result<(), Box<dyn Error>> {
    let codec = OutputCodec::default();
    for case in fixture()?.cases {
        let original_text = serde_json::to_string(&case.value)?;
        let planned = codec.encode(case.value);
        assert_eq!(
            matches!(planned.plan, PresentationPlan::Compact { .. }),
            case.expect_compact,
            "unexpected plan for {}: {:?}",
            case.name,
            planned.plan
        );
        if let PresentationPlan::Compact {
            full_bytes,
            compact_bytes,
            ..
        } = planned.plan
        {
            assert!(compact_bytes < full_bytes, "{} did not shrink", case.name);
            let wire = planned.output.clone().into_wire_value()?;
            assert_eq!(serde_json::to_vec(&wire)?.len(), compact_bytes);
        }
        let decoded = codec.decode(planned.output)?;
        assert_eq!(
            serde_json::to_string(&decoded)?,
            original_text,
            "serialized reconstruction mismatch for {}",
            case.name
        );
    }
    Ok(())
}

#[test]
fn preserves_signed_zero_wide_integer_and_positional_nulls() -> Result<(), Box<dyn Error>> {
    let case = fixture()?
        .cases
        .into_iter()
        .find(|case| case.name == "positive_defaults_preserve_edge_values")
        .ok_or_else(|| io::Error::other("edge-value fixture missing"))?;
    let decoded =
        OutputCodec::default().decode(OutputCodec::default().encode(case.value).output)?;
    let params = decoded
        .pointer("/nodes/0/params")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("params missing"))?;
    let negative_zero = params
        .get(3)
        .and_then(|row| row.get("val"))
        .and_then(Value::as_f64)
        .ok_or_else(|| io::Error::other("negative zero missing"))?;
    assert_eq!(negative_zero.to_bits(), (-0.0_f64).to_bits());
    let wide = params
        .get(4)
        .and_then(|row| row.get("val"))
        .and_then(Value::as_u64)
        .ok_or_else(|| io::Error::other("wide integer missing"))?;
    assert_eq!(wide, u64::MAX);
    assert_eq!(decoded.pointer("/nodes/0/inputs/0"), Some(&Value::Null));
    assert_eq!(decoded.pointer("/nodes/0/inputs/2"), Some(&Value::Null));
    assert_eq!(serde_json::to_string(&params[3]["val"])?, "-0.0");
    Ok(())
}

#[test]
fn negative_statuses_are_never_implicit_defaults() -> Result<(), Box<dyn Error>> {
    let value = json!({"params": [
        {"name":"A", "mode":"CONSTANT", "evaluation":{"available":true}, "warnings":[]},
        {"name":"B", "mode":"CONSTANT", "evaluation":{"available":false, "code":"unavailable"}, "warnings":["bad"]},
        {"name":"C", "mode":"CONSTANT", "evaluation":{"available":true}, "ok":false}
    ]});
    let planned = OutputCodec::default().encode(value.clone());
    let compact = match &planned.output {
        EncodedOutput::Compact(compact) => compact,
        EncodedOutput::Full(_) => {
            assert_eq!(OutputCodec::default().decode(planned.output)?, value);
            return Ok(());
        }
    };
    for transform in &compact.transforms {
        if let Transform::SharedDefaults { defaults, .. } = transform {
            assert!(!defaults.contains_key("warnings"));
            assert!(!defaults.contains_key("ok"));
            assert!(!defaults.contains_key("evaluation"));
        }
    }
    let decoded = OutputCodec::default().decode(planned.output)?;
    assert_eq!(
        serde_json::to_string(&decoded)?,
        serde_json::to_string(&value)?
    );
    Ok(())
}

#[test]
fn literal_control_shaped_data_is_not_interpreted() -> Result<(), Box<dyn Error>> {
    let value = json!({
        "codec": {"name": CODEC_NAME, "version": CODEC_VERSION},
        "data": {"$ref":"literal", "kind":"shared_members"},
        "transforms": [{"kind":"names_vector", "path":["literal"]}],
        "params": (0..20).map(|index| json!({"name": format!("P{index}")})).collect::<Vec<_>>()
    });
    let planned = OutputCodec::default().encode(value.clone());
    let decoded = OutputCodec::default().decode(planned.output)?;
    assert_eq!(
        serde_json::to_string(&decoded)?,
        serde_json::to_string(&value)?
    );
    Ok(())
}

#[test]
fn malformed_duplicate_overlap_and_prototype_paths_are_rejected() {
    let codec = OutputCodec::default();
    let duplicate = compact(
        json!({"a":["x","y"]}),
        vec![
            Transform::NamesVector {
                path: vec![key("a")],
            },
            Transform::NamesVector {
                path: vec![key("a")],
            },
        ],
    );
    assert!(matches!(
        codec.decode_compact(duplicate),
        Err(CodecError::Malformed(message)) if message.contains("overlapping")
    ));

    let overlap = compact(
        json!({"a":{"b":["x","y"]}}),
        vec![
            Transform::NamesVector {
                path: vec![key("a")],
            },
            Transform::NamesVector {
                path: vec![key("a"), key("b")],
            },
        ],
    );
    assert!(matches!(
        codec.decode_compact(overlap),
        Err(CodecError::Malformed(message)) if message.contains("overlapping")
    ));

    let prototype = compact(
        json!({"__proto__":["x","y"]}),
        vec![Transform::NamesVector {
            path: vec![key("__proto__")],
        }],
    );
    assert!(matches!(
        codec.decode_compact(prototype),
        Err(CodecError::Malformed(message)) if message.contains("prototype-like")
    ));
}

#[test]
fn unsupported_codec_version_and_unknown_control_fields_fail_explicitly(
) -> Result<(), Box<dyn Error>> {
    let codec = OutputCodec::default();
    let mut wrong_name = compact(
        json!({"p":["A","B"]}),
        vec![Transform::NamesVector {
            path: vec![key("p")],
        }],
    );
    wrong_name.codec.name = "other.codec".to_owned();
    assert!(matches!(
        codec.decode_compact(wrong_name),
        Err(CodecError::UnsupportedCodec(name)) if name == "other.codec"
    ));

    let mut wrong_version = compact(
        json!({"p":["A","B"]}),
        vec![Transform::NamesVector {
            path: vec![key("p")],
        }],
    );
    wrong_version.codec.version = CODEC_VERSION + 1;
    assert_eq!(
        codec.decode_compact(wrong_version),
        Err(CodecError::UnsupportedVersion(CODEC_VERSION + 1))
    );

    let unknown = json!({
        "codec":{"name":CODEC_NAME,"version":CODEC_VERSION,"future":true},
        "data":{"p":["A","B"]},
        "transforms":[{"kind":"names_vector","path":["p"]}]
    });
    assert!(matches!(
        CompactOutput::from_wire_value(unknown),
        Err(CodecError::Malformed(_))
    ));
    Ok(())
}

#[test]
fn decoder_rejects_negative_defaults_and_overwrites() {
    let codec = OutputCodec::default();
    let mut negative = Map::new();
    negative.insert("evaluation".to_owned(), json!({"available":false}));
    let bad_default = compact(
        json!({"params":[{"name":"A"},{"name":"B"}]}),
        vec![Transform::SharedDefaults {
            path: vec![key("params")],
            defaults: negative,
        }],
    );
    assert!(matches!(
        codec.decode_compact(bad_default),
        Err(CodecError::Malformed(message)) if message.contains("negative")
    ));

    let mut defaults = Map::new();
    defaults.insert("mode".to_owned(), Value::String("CONSTANT".to_owned()));
    let overwrite = compact(
        json!({"params":[{"name":"A","mode":"EXPRESSION"}]}),
        vec![Transform::SharedDefaults {
            path: vec![key("params")],
            defaults,
        }],
    );
    assert!(matches!(
        codec.decode_compact(overwrite),
        Err(CodecError::Malformed(message)) if message.contains("overwrite")
    ));
}

#[test]
fn traversal_and_transform_limits_fall_back_without_trimming() -> Result<(), Box<dyn Error>> {
    let value = json!({"a":{"b":{"c":{"params":[{"name":"A"},{"name":"B"}]}}}});
    let limits = CodecLimits {
        max_depth: 2,
        ..CodecLimits::default()
    };
    let planned = OutputCodec::new(limits).encode(value.clone());
    assert!(matches!(
        planned.plan,
        PresentationPlan::Full {
            reason: FullReason::TraversalBudget,
            ..
        }
    ));
    assert_eq!(planned.output.into_wire_value()?, value);

    let value = json!({
        "a":[{"name":"A"},{"name":"B"}],
        "b":[{"name":"C"},{"name":"D"}]
    });
    let limits = CodecLimits {
        max_transforms: 1,
        ..CodecLimits::default()
    };
    let planned = OutputCodec::new(limits).encode(value.clone());
    assert!(matches!(
        planned.plan,
        PresentationPlan::Full {
            reason: FullReason::TransformBudget,
            ..
        }
    ));
    assert_eq!(planned.output.into_wire_value()?, value);
    Ok(())
}

#[test]
fn deterministic_property_style_round_trip() -> Result<(), Box<dyn Error>> {
    let codec = OutputCodec::default();
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    for case_index in 0..256_u64 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let row_count = 2 + usize::try_from(state % 24)?;
        let names_only = state & 1 == 0;
        let mut rows = Vec::with_capacity(row_count);
        for row_index in 0..row_count {
            let name = format!("P{}_{}", case_index % 7, row_index % 5);
            if names_only {
                rows.push(json!({"name":name}));
            } else {
                let val = match row_index % 5 {
                    0 => Value::Null,
                    1 => Value::Bool(false),
                    2 => json!(0),
                    3 => json!(-0.0),
                    _ => json!(u64::MAX - case_index),
                };
                rows.push(json!({
                    "name":name,
                    "mode":"CONSTANT",
                    "val":val,
                    "evaluation":{"available":true},
                    "extension":{"case":case_index,"row":row_index}
                }));
            }
        }
        let value = json!({
            "ok":true,
            "nodes":[{
                "ok": case_index % 11 != 0,
                "path":format!("/synthetic/{case_index}"),
                "params":rows,
                "inputs":[null,{"path":"/a"},null],
                "warnings": if case_index % 13 == 0 { vec!["warning"] } else { Vec::<&str>::new() },
                "unknown":{"kind":"shared_members","$ref":"literal"}
            }]
        });
        let original = serde_json::to_string(&value)?;
        let first = codec.encode(value);
        let plan_again = codec.plan(match &first.output {
            EncodedOutput::Full(value) => value,
            EncodedOutput::Compact(_) => {
                let decoded = codec.decode(first.output.clone())?;
                let second = codec.plan(&decoded);
                assert_eq!(second, codec.plan(&decoded));
                let reconstructed = serde_json::to_string(&decoded)?;
                assert_eq!(reconstructed, original);
                continue;
            }
        });
        assert_eq!(first.plan, plan_again);
        let decoded = codec.decode(first.output)?;
        assert_eq!(serde_json::to_string(&decoded)?, original);
    }
    Ok(())
}
