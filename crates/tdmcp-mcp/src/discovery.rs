//! Targeted discovery without reducing ordinary MCP tool-list visibility.
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::resources::ResourceProvider;
use crate::schema::input_schema_for;
use crate::template::RenderMode;
use crate::tools::{tool_descriptors, ToolName};

fn default_schema() -> bool {
    true
}

fn bounded_names<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    let names = Option::<Vec<String>>::deserialize(deserializer)?;
    if names
        .as_ref()
        .is_some_and(|names| names.len() > 64 || names.iter().any(|name| name.len() > 128))
    {
        return Err(serde::de::Error::custom(
            "names supports at most 64 exact names of at most 128 bytes",
        ));
    }
    Ok(names)
}

/// Explicit discovery projections. Omitted arguments preserve the full legacy catalog.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DescribeToolsParams {
    /// Exact case-sensitive names, in requested order. Omitted/null selects all; [] selects none. At most 64 names.
    #[serde(default, deserialize_with = "bounded_names")]
    pub names: Option<Vec<String>>,
    /// Include complete derived input schemas (default true). False is an explicit projection, not compressed schemas.
    #[serde(default = "default_schema")]
    pub include_schema: bool,
    /// Include validated illustrative requests (default false); examples never authorize their execution.
    #[serde(default)]
    pub include_examples: bool,
    /// Include SHA256 revisions of the owning backend catalog and embedded resources (default false).
    #[serde(default)]
    pub include_revisions: bool,
}

/// One requested tool surface. Omitted schemas/examples are explicitly unrequested.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryTool {
    name: &'static str,
    description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    examples: Option<Vec<DiscoveryExample>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryExample {
    id: String,
    arguments: Value,
    context: String,
    expected: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Selection {
    requested: Vec<String>,
    missing: Vec<String>,
    complete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    input_schema: bool,
    examples: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Revisions {
    algorithm: &'static str,
    encoding: &'static str,
    catalog: String,
    resources: String,
    examples: String,
    package_version: &'static str,
    scope: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryResponse {
    tools: Vec<DiscoveryTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<Selection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projection: Option<Projection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revisions: Option<Revisions>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn examples() -> Result<BTreeMap<String, Vec<DiscoveryExample>>, serde_json::Error> {
    let registry: Value = serde_json::from_str(include_str!("../../../skills/EXAMPLES.json"))?;
    let mut result: BTreeMap<String, Vec<DiscoveryExample>> = BTreeMap::new();
    if let Some(blocks) = registry["blocks"].as_array() {
        for block in blocks {
            let Some(tool) = block["tool"].as_str() else {
                continue;
            };
            // Registry validity and decoder parity are checked by the authoring gate.
            if let (Some(id), Some(arguments), Some(context), Some(expected)) = (
                block["id"].as_str(),
                block.get("arguments"),
                block["context"].as_str(),
                block["expected"].as_str(),
            ) {
                result
                    .entry(tool.to_owned())
                    .or_default()
                    .push(DiscoveryExample {
                        id: id.to_owned(),
                        arguments: arguments.clone(),
                        context: context.to_owned(),
                        expected: expected.to_owned(),
                    });
            }
        }
    }
    Ok(result)
}

/// Render requested current backend descriptors without calling TD or hiding tools.
/// Hashes identify captured embedded catalog bytes, not loaded bridge support.
pub fn describe(params: DescribeToolsParams) -> Result<Value, serde_json::Error> {
    let requested = params.names.clone().unwrap_or_else(|| {
        ToolName::ALL
            .iter()
            .map(|tool| tool.wire_str().to_owned())
            .collect()
    });
    let mut example_map = if params.include_examples {
        examples()?
    } else {
        BTreeMap::new()
    };
    let mut tools = Vec::with_capacity(requested.len());
    let mut missing = Vec::new();
    for name in &requested {
        let Some(tool) = ToolName::from_wire(name) else {
            missing.push(name.clone());
            continue;
        };
        // Do not derive every tool's schema just to answer a single-name query.
        tools.push(DiscoveryTool {
            name: tool.wire_str(),
            description: tool.description(),
            input_schema: params.include_schema.then(|| input_schema_for(tool)),
            examples: params.include_examples.then(|| {
                // Reconstruct duplicate requested entries, without consuming their examples.
                example_map
                    .entry(name.clone())
                    .or_default()
                    .iter()
                    .map(|example| DiscoveryExample {
                        id: example.id.clone(),
                        arguments: example.arguments.clone(),
                        context: example.context.clone(),
                        expected: example.expected.clone(),
                    })
                    .collect()
            }),
        });
    }
    let selection = params.names.map(|_| Selection {
        requested,
        complete: missing.is_empty(),
        missing,
    });
    let revisions = if params.include_revisions {
        let provider = ResourceProvider::from_embedded()
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        let mut rendered = provider
            .engine()
            .render_all(RenderMode::Mcp)
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        rendered.sort_by(|left, right| left.0.cmp(&right.0));
        Some(Revisions {
            algorithm: "sha256",
            encoding: "lower_hex",
            catalog: digest(&serde_json::to_vec(&tool_descriptors())?),
            resources: digest(&serde_json::to_vec(&(crate::MANIFEST_YAML, rendered))?),
            examples: digest(include_bytes!("../../../skills/EXAMPLES.json")),
            package_version: env!("CARGO_PKG_VERSION"),
            scope: "owning-backend-embedded-catalog; not loaded-bridge support or scene freshness",
        })
    } else {
        None
    };
    serde_json::to_value(DiscoveryResponse {
        tools,
        selection,
        projection: (!params.include_schema || params.include_examples).then_some(Projection {
            input_schema: params.include_schema,
            examples: params.include_examples,
        }),
        revisions,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn query(value: Value) -> Value {
        describe(serde_json::from_value(value).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn actual_dispatch_validates_and_does_not_need_a_bridge() {
        let registry = std::sync::Arc::new(tokio::sync::Mutex::new(tdmcp_core::PidRegistry::new()));
        let catalog = tdmcp_diagnostics::Catalog::fallback();
        let bridge =
            crate::testing::FakeBridgeRpc::failing(crate::testing::BridgeRpcFailure::Timeout, 1);
        let value = crate::tools::dispatch_tool(
            &registry,
            &catalog,
            &bridge,
            "describe_tools",
            json!({"names":["inspect"],"includeSchema":false}),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(value["tools"].as_array().unwrap().len(), 1);
        assert!(value["tools"][0].get("inputSchema").is_none());
        assert!(crate::tools::dispatch_tool(
            &registry,
            &catalog,
            &bridge,
            "describe_tools",
            json!({"names":"inspect"}),
            None,
            None
        )
        .await
        .is_err());
        let default = crate::tools::dispatch_tool(
            &registry,
            &catalog,
            &bridge,
            "describe_tools",
            Value::Null,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(default, json!({"tools":tool_descriptors()}));
    }

    #[test]
    fn default_catalog_is_exactly_legacy() {
        assert_eq!(query(json!({})), json!({"tools": tool_descriptors()}));
        assert_eq!(query(json!({"names": null})), query(json!({})));
        assert_eq!(query(json!({"names": []}))["tools"], json!([]));
    }

    #[test]
    fn selection_preserves_order_duplicates_and_missing_names() {
        let value = query(json!({"names": ["capture", "not_a_tool", "inspect", "capture"]}));
        assert_eq!(value["tools"][0]["name"], "capture");
        assert_eq!(value["tools"][1]["name"], "inspect");
        assert_eq!(value["tools"][2]["name"], "capture");
        assert_eq!(value["selection"]["missing"], json!(["not_a_tool"]));
        assert_eq!(value["selection"]["complete"], false);
    }

    #[test]
    fn projected_fields_remain_distinct_from_empty_requested_examples() {
        let value =
            query(json!({"names": ["kill_td"], "includeSchema": false, "includeExamples": true}));
        assert!(value["tools"][0].get("inputSchema").is_none());
        assert_eq!(value["tools"][0]["examples"], json!([]));
        assert_eq!(
            value["projection"],
            json!({"inputSchema": false, "examples": true})
        );
        let value = query(json!({"names": ["inspect"], "includeExamples": true}));
        assert!(!value["tools"][0]["examples"].as_array().unwrap().is_empty());
    }

    #[test]
    fn revisions_are_reproducible_and_scoped_to_backend_bytes() {
        let params = json!({"names": [], "includeRevisions": true});
        let first = query(params.clone());
        assert_eq!(first, query(params));
        assert_eq!(first["revisions"]["catalog"].as_str().unwrap().len(), 64);
        assert!(first["revisions"]["scope"]
            .as_str()
            .unwrap()
            .contains("not loaded-bridge"));
    }

    #[test]
    fn malformed_and_unbounded_inputs_are_rejected() {
        for value in [
            json!({"names": "inspect"}),
            json!({"names": vec!["inspect";65]}),
            json!({"names": ["x".repeat(129)]}),
            json!({"includeSchema": null}),
            json!({"includeExamples": 0}),
            json!({"unknown": true}),
        ] {
            assert!(serde_json::from_value::<DescribeToolsParams>(value).is_err());
        }
    }
}
