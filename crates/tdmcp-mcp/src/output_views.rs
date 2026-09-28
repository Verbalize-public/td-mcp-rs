//! Bounded, lossless presentation views for already-captured JSON output.
//!
//! This module is deliberately independent of tool dispatch and evidence acquisition: it
//! transforms only an owned [`serde_json::Value`] and never calls TouchDesigner or evaluates
//! project data. Legacy output is returned byte-for-byte equivalent at the JSON-value level
//! when compact encoding is unavailable or not strictly smaller.
//!
//! Compact wire format version 1 is:
//!
//! ```json
//! {
//!   "codec": {"name": "output-views", "version": 1},
//!   "data": {"nodes": [{"params": ["Gain", "Reset"]}]},
//!   "transforms": [
//!     {"kind": "names_vector", "path": ["nodes", 0, "params"]}
//!   ]
//! }
//! ```
//!
//! `data` contains only transformed user data. `codec` and `transforms` are a separate control
//! plane; strings or object keys inside `data` are never interpreted as references or markers.
//! Consumers must enter compact decoding explicitly (for example after negotiating this view),
//! rather than guessing from keys in arbitrary legacy JSON. Version 1 supports only names
//! vectors, positive parameter defaults, and identical API-help member rosters. It performs no
//! projection, omission, external reference, source interning, or summary.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::io::{self, Write};
use thiserror::Error;

/// Stable compact codec name.
///
/// Deliberately outside the `tdmcp.*` diagnostics namespace: that namespace is
/// reserved for error codes in `diagnostics/catalog.yaml`, and a client that
/// scanned values by prefix must not be able to mistake a codec name for a
/// diagnostic code.
pub const CODEC_NAME: &str = "output-views";
/// Current compact codec version.
pub const CODEC_VERSION: u32 = 1;

const MODE_FIELD: &str = "mode";
const EVALUATION_FIELD: &str = "evaluation";
const MEMBERS_FIELD: &str = "members";
const NAME_FIELD: &str = "name";

/// Admission limits for codec traversal, control metadata, and size accounting.
///
/// Exceeding an encode limit selects the exact full value. It never trims the value. Exceeding
/// a decode limit is an error because accepting an incompletely validated compact document would
/// be unsafe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecLimits {
    /// Maximum nested array/object depth visited.
    pub max_depth: usize,
    /// Maximum total JSON values visited.
    pub max_values: usize,
    /// Maximum emitted transforms.
    pub max_transforms: usize,
    /// Maximum identical roster locations considered.
    pub max_roster_paths: usize,
    /// Maximum paths carried by one shared-roster transform.
    pub max_paths_per_transform: usize,
    /// Maximum path segments accepted in one transform target.
    pub max_path_segments: usize,
    /// Maximum serialized bytes counted for either representation.
    pub max_serialized_bytes: usize,
}

impl Default for CodecLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_values: 250_000,
            max_transforms: 2_048,
            max_roster_paths: 256,
            max_paths_per_transform: 256,
            max_path_segments: 64,
            max_serialized_bytes: 32 * 1024 * 1024,
        }
    }
}

/// Why the planner kept the legacy representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullReason {
    /// No supported repeated structure was present.
    NoTransforms,
    /// The complete compact document was not strictly smaller.
    NotSmaller,
    /// Input traversal exceeded the configured depth/value admission budget.
    TraversalBudget,
    /// Serialized size accounting exceeded the configured byte budget.
    SerializationBudget,
    /// The number of candidate paths or transforms exceeded its configured bound.
    TransformBudget,
}

/// Planner decision and complete serialized-size accounting when admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationPlan {
    /// Preserve the legacy value exactly.
    Full {
        /// Counted compact JSON byte length of the full value, if within the byte budget.
        full_bytes: Option<usize>,
        /// Counted compact JSON byte length of the rejected candidate, when one was built.
        candidate_bytes: Option<usize>,
        /// Reason compact output was not selected.
        reason: FullReason,
    },
    /// Emit the versioned compact document because its complete serialized form is smaller.
    Compact {
        /// Counted compact JSON byte length of the legacy value.
        full_bytes: usize,
        /// Counted compact JSON byte length including codec header and transform legend.
        compact_bytes: usize,
        /// Number of control transforms in the compact document.
        transforms: usize,
    },
}

/// A planned output. `Full` serializes as the original value without an added envelope.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodedOutput {
    /// Unchanged legacy JSON.
    Full(Value),
    /// Explicitly versioned lossless compact JSON.
    Compact(CompactOutput),
}

impl EncodedOutput {
    /// Convert the selected representation to its wire JSON value.
    pub fn into_wire_value(self) -> Result<Value, CodecError> {
        match self {
            Self::Full(value) => Ok(value),
            Self::Compact(value) => serde_json::to_value(value)
                .map_err(|error| CodecError::Serialization(error.to_string())),
        }
    }
}

/// Result of planning and encoding in one pass.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedOutput {
    /// Size-based planner decision.
    pub plan: PresentationPlan,
    /// Selected full or compact representation.
    pub output: EncodedOutput,
}

/// Explicit codec header. Unknown fields are rejected rather than ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecHeader {
    /// Stable codec name.
    pub name: String,
    /// Exact codec version.
    pub version: u32,
}

/// One typed path segment. Strings are object keys and integers are array indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSegment {
    /// Object key.
    Key(String),
    /// Array index.
    Index(usize),
}

impl fmt::Display for PathSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => write!(formatter, ".{key}"),
            Self::Index(index) => write!(formatter, "[{index}]"),
        }
    }
}

/// Lossless transform legend for [`CompactOutput::data`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transform {
    /// Replace an array of exact `{name: string}` records with a vector of strings.
    NamesVector {
        /// Path to the transformed array.
        path: Vec<PathSegment>,
    },
    /// Remove explicitly declared positive defaults from every record in one array.
    SharedDefaults {
        /// Path to the record array.
        path: Vec<PathSegment>,
        /// Defaults restored into every row. Version 1 permits only `mode: "CONSTANT"` and
        /// `evaluation: {available: true}`.
        defaults: Map<String, Value>,
    },
    /// Store one identical API-help member roster for all declared result paths.
    SharedMembers {
        /// Paths where the `members` key was removed.
        paths: Vec<Vec<PathSegment>>,
        /// Exact ordered, duplicate-preserving roster.
        members: Vec<String>,
    },
}

/// Complete version-1 compact document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactOutput {
    /// Codec identity and version.
    pub codec: CodecHeader,
    /// Transformed captured JSON. Literal contents are inert.
    pub data: Value,
    /// Complete transform legend needed to reconstruct the legacy value.
    pub transforms: Vec<Transform>,
}

impl CompactOutput {
    /// Parse a value only when the caller already expects a compact document.
    ///
    /// This intentionally does not auto-detect marker keys in arbitrary legacy data.
    pub fn from_wire_value(value: Value) -> Result<Self, CodecError> {
        serde_json::from_value(value).map_err(|error| CodecError::Malformed(error.to_string()))
    }
}

/// Codec failure. Encode admission failures normally become a full fallback; decode failures
/// are explicit because silently discarding or partially applying data is forbidden.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Codec name is not supported.
    #[error("unsupported output codec {0:?}")]
    UnsupportedCodec(String),
    /// Codec version is not supported.
    #[error("unsupported output codec version {0}")]
    UnsupportedVersion(u32),
    /// Compact document is malformed or violates transform invariants.
    #[error("malformed compact output: {0}")]
    Malformed(String),
    /// Compact document exceeds decoder admission limits.
    #[error("compact output exceeds {0} budget")]
    BudgetExceeded(&'static str),
    /// JSON serialization failed.
    #[error("JSON serialization failed: {0}")]
    Serialization(String),
}

/// Bounded version-1 lossless presentation codec.
#[derive(Debug, Clone, Copy)]
pub struct OutputCodec {
    limits: CodecLimits,
}

impl Default for OutputCodec {
    fn default() -> Self {
        Self::new(CodecLimits::default())
    }
}

impl OutputCodec {
    /// Construct a codec with explicit admission limits.
    #[must_use]
    pub const fn new(limits: CodecLimits) -> Self {
        Self { limits }
    }

    /// Return configured admission limits.
    #[must_use]
    pub const fn limits(&self) -> CodecLimits {
        self.limits
    }

    /// Plan a representation without retaining the encoded candidate.
    ///
    /// This performs the same bounded work as [`Self::encode`]. Prefer `encode` when the output is
    /// also needed so planning is not repeated.
    #[must_use]
    pub fn plan(&self, source: &Value) -> PresentationPlan {
        self.encode(source.clone()).plan
    }

    /// Plan and encode an owned captured value.
    ///
    /// Compact is selected only when counting serialization proves that the complete document,
    /// including header and transform legend, is strictly smaller. Any encode budget exhaustion
    /// returns `EncodedOutput::Full(source)` unchanged.
    #[must_use]
    pub fn encode(&self, source: Value) -> PlannedOutput {
        let full_bytes = match admitted_serialized_len(&source, self.limits) {
            Ok(bytes) => bytes,
            Err(Admission::Traversal) => {
                return full_fallback(source, None, None, FullReason::TraversalBudget);
            }
            Err(Admission::Serialization) => {
                return full_fallback(source, None, None, FullReason::SerializationBudget);
            }
            Err(Admission::Transform) => {
                return full_fallback(source, None, None, FullReason::TransformBudget);
            }
        };

        let mut compact = CompactOutput {
            codec: CodecHeader {
                name: CODEC_NAME.to_owned(),
                version: CODEC_VERSION,
            },
            data: source.clone(),
            transforms: Vec::new(),
        };
        let mut path = Vec::new();
        let mut roster_paths = Vec::new();
        if transform_value(
            &mut compact.data,
            &mut path,
            &mut compact.transforms,
            &mut roster_paths,
            self.limits,
        )
        .is_err()
            || factor_member_rosters(
                &mut compact.data,
                &mut compact.transforms,
                &roster_paths,
                self.limits,
            )
            .is_err()
        {
            return full_fallback(source, Some(full_bytes), None, FullReason::TransformBudget);
        }

        if compact.transforms.is_empty() {
            return full_fallback(source, Some(full_bytes), None, FullReason::NoTransforms);
        }

        let compact_bytes = match counted_serialized_len(&compact, self.limits.max_serialized_bytes)
        {
            Ok(bytes) => bytes,
            Err(()) => {
                return full_fallback(
                    source,
                    Some(full_bytes),
                    None,
                    FullReason::SerializationBudget,
                );
            }
        };
        if compact_bytes >= full_bytes {
            return full_fallback(
                source,
                Some(full_bytes),
                Some(compact_bytes),
                FullReason::NotSmaller,
            );
        }

        PlannedOutput {
            plan: PresentationPlan::Compact {
                full_bytes,
                compact_bytes,
                transforms: compact.transforms.len(),
            },
            output: EncodedOutput::Compact(compact),
        }
    }

    /// Decode a typed selected output.
    pub fn decode(&self, encoded: EncodedOutput) -> Result<Value, CodecError> {
        match encoded {
            EncodedOutput::Full(value) => Ok(value),
            EncodedOutput::Compact(compact) => self.decode_compact(compact),
        }
    }

    /// Decode an explicitly selected compact document.
    ///
    /// All paths and transform payloads are validated before any transform is applied. Duplicate
    /// or ancestor-overlapping targets, prototype-like path keys, unexpected existing fields,
    /// negative defaults, unknown codecs, and unsupported versions are rejected.
    pub fn decode_compact(&self, mut compact: CompactOutput) -> Result<Value, CodecError> {
        if compact.codec.name != CODEC_NAME {
            return Err(CodecError::UnsupportedCodec(compact.codec.name));
        }
        if compact.codec.version != CODEC_VERSION {
            return Err(CodecError::UnsupportedVersion(compact.codec.version));
        }
        if compact.transforms.is_empty() {
            return Err(CodecError::Malformed(
                "compact document has no transforms".to_owned(),
            ));
        }
        if compact.transforms.len() > self.limits.max_transforms {
            return Err(CodecError::BudgetExceeded("transform count"));
        }
        scan_value(&compact.data, self.limits)?;
        validate_transforms(&compact.transforms, self.limits)?;

        for transform in compact.transforms {
            match transform {
                Transform::NamesVector { path } => decode_names(&mut compact.data, &path)?,
                Transform::SharedDefaults { path, defaults } => {
                    decode_defaults(&mut compact.data, &path, defaults)?;
                }
                Transform::SharedMembers { paths, members } => {
                    for path in paths {
                        insert_members(&mut compact.data, &path, &members)?;
                    }
                }
            }
        }
        scan_value(&compact.data, self.limits)?;
        Ok(compact.data)
    }
}

fn full_fallback(
    source: Value,
    full_bytes: Option<usize>,
    candidate_bytes: Option<usize>,
    reason: FullReason,
) -> PlannedOutput {
    PlannedOutput {
        plan: PresentationPlan::Full {
            full_bytes,
            candidate_bytes,
            reason,
        },
        output: EncodedOutput::Full(source),
    }
}

#[derive(Debug, Clone, Copy)]
enum Admission {
    Traversal,
    Serialization,
    Transform,
}

fn admitted_serialized_len(value: &Value, limits: CodecLimits) -> Result<usize, Admission> {
    scan_value(value, limits).map_err(|_| Admission::Traversal)?;
    counted_serialized_len(value, limits.max_serialized_bytes)
        .map_err(|()| Admission::Serialization)
}

fn scan_value(value: &Value, limits: CodecLimits) -> Result<(), CodecError> {
    fn visit(
        value: &Value,
        depth: usize,
        count: &mut usize,
        limits: CodecLimits,
    ) -> Result<(), CodecError> {
        if depth > limits.max_depth {
            return Err(CodecError::BudgetExceeded("depth"));
        }
        *count = count
            .checked_add(1)
            .ok_or(CodecError::BudgetExceeded("value count"))?;
        if *count > limits.max_values {
            return Err(CodecError::BudgetExceeded("value count"));
        }
        match value {
            Value::Array(values) => {
                for child in values {
                    visit(child, depth + 1, count, limits)?;
                }
            }
            Value::Object(values) => {
                for child in values.values() {
                    visit(child, depth + 1, count, limits)?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
        Ok(())
    }

    let mut count = 0;
    visit(value, 0, &mut count, limits)
}

struct CountingWriter {
    bytes: usize,
    limit: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::other("serialized size overflow"))?;
        if next > self.limit {
            return Err(io::Error::other("serialized size budget exceeded"));
        }
        self.bytes = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn counted_serialized_len<T: Serialize>(value: &T, limit: usize) -> Result<usize, ()> {
    let mut writer = CountingWriter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| ())?;
    Ok(writer.bytes)
}

fn transform_value(
    value: &mut Value,
    path: &mut Vec<PathSegment>,
    transforms: &mut Vec<Transform>,
    roster_paths: &mut Vec<Vec<PathSegment>>,
    limits: CodecLimits,
) -> Result<(), Admission> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                path.push(PathSegment::Key(key.clone()));
                transform_value(child, path, transforms, roster_paths, limits)?;
                path.pop();
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter_mut().enumerate() {
                path.push(PathSegment::Index(index));
                transform_value(child, path, transforms, roster_paths, limits)?;
                path.pop();
            }
            if path_is_safe(path) && is_member_roster_path(path) && is_string_roster(array) {
                if roster_paths.len() >= limits.max_roster_paths {
                    return Err(Admission::Transform);
                }
                roster_paths.push(path.clone());
            }
            if !path_is_safe(path) {
                return Ok(());
            }
            if let Some(names) = take_names_vector(array) {
                push_transform(
                    transforms,
                    Transform::NamesVector { path: path.clone() },
                    limits,
                )?;
                *array = names.into_iter().map(Value::String).collect();
                return Ok(());
            }
            if let Some(defaults) = shared_positive_defaults(array) {
                for row in array {
                    if let Some(object) = row.as_object_mut() {
                        for key in defaults.keys() {
                            object.remove(key);
                        }
                    }
                }
                push_transform(
                    transforms,
                    Transform::SharedDefaults {
                        path: path.clone(),
                        defaults,
                    },
                    limits,
                )?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn push_transform(
    transforms: &mut Vec<Transform>,
    transform: Transform,
    limits: CodecLimits,
) -> Result<(), Admission> {
    if transforms.len() >= limits.max_transforms {
        return Err(Admission::Transform);
    }
    transforms.push(transform);
    Ok(())
}

fn take_names_vector(array: &[Value]) -> Option<Vec<String>> {
    if array.len() < 2 {
        return None;
    }
    let mut names = Vec::with_capacity(array.len());
    for row in array {
        let object = row.as_object()?;
        if object.len() != 1 {
            return None;
        }
        names.push(object.get(NAME_FIELD)?.as_str()?.to_owned());
    }
    Some(names)
}

fn shared_positive_defaults(array: &[Value]) -> Option<Map<String, Value>> {
    if array.len() < 2 || !array.iter().all(Value::is_object) {
        return None;
    }
    let mut defaults = Map::new();
    let constant = Value::String("CONSTANT".to_owned());
    if array.iter().all(|row| {
        row.get(MODE_FIELD)
            .is_some_and(|value| exact_value_eq(value, &constant))
    }) {
        defaults.insert(MODE_FIELD.to_owned(), constant);
    }
    let available = serde_json::json!({"available": true});
    if array.iter().all(|row| {
        row.get(EVALUATION_FIELD)
            .is_some_and(|value| exact_value_eq(value, &available))
    }) {
        defaults.insert(EVALUATION_FIELD.to_owned(), available);
    }
    (!defaults.is_empty()).then_some(defaults)
}

fn is_member_roster_path(path: &[PathSegment]) -> bool {
    matches!(path.last(), Some(PathSegment::Key(key)) if key == MEMBERS_FIELD)
}

fn is_string_roster(array: &[Value]) -> bool {
    array.len() >= 2 && array.iter().all(Value::is_string)
}

fn factor_member_rosters(
    data: &mut Value,
    transforms: &mut Vec<Transform>,
    roster_paths: &[Vec<PathSegment>],
    limits: CodecLimits,
) -> Result<(), Admission> {
    let mut used = vec![false; roster_paths.len()];
    for index in 0..roster_paths.len() {
        if used[index] {
            continue;
        }
        let Some(base) = get_at(data, &roster_paths[index]) else {
            continue;
        };
        let mut group = vec![index];
        for candidate in (index + 1)..roster_paths.len() {
            if !used[candidate]
                && get_at(data, &roster_paths[candidate])
                    .is_some_and(|value| exact_value_eq(base, value))
            {
                group.push(candidate);
            }
        }
        if group.len() < 2 {
            continue;
        }
        if group.len() > limits.max_paths_per_transform {
            return Err(Admission::Transform);
        }
        for member in &group {
            used[*member] = true;
        }
        let Some(Value::Array(roster)) = remove_at(data, &roster_paths[group[0]]) else {
            return Err(Admission::Transform);
        };
        let mut members = Vec::with_capacity(roster.len());
        for value in roster {
            let Value::String(member) = value else {
                return Err(Admission::Transform);
            };
            members.push(member);
        }
        let mut paths = vec![roster_paths[group[0]].clone()];
        for member in group.into_iter().skip(1) {
            let Some(removed) = remove_at(data, &roster_paths[member]) else {
                return Err(Admission::Transform);
            };
            if !matches!(removed, Value::Array(_)) {
                return Err(Admission::Transform);
            }
            paths.push(roster_paths[member].clone());
        }
        push_transform(
            transforms,
            Transform::SharedMembers { paths, members },
            limits,
        )?;
    }
    Ok(())
}

fn exact_value_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => exact_number_eq(left, right),
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| exact_value_eq(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| exact_value_eq(value, other))
                })
        }
        _ => false,
    }
}

fn exact_number_eq(left: &serde_json::Number, right: &serde_json::Number) -> bool {
    match (
        left.as_i64(),
        left.as_u64(),
        left.as_f64(),
        right.as_i64(),
        right.as_u64(),
        right.as_f64(),
    ) {
        (Some(left), _, _, Some(right), _, _) => left == right,
        (None, Some(left), _, None, Some(right), _) => left == right,
        (None, None, Some(left), None, None, Some(right)) => left.to_bits() == right.to_bits(),
        _ => false,
    }
}

fn validate_transforms(transforms: &[Transform], limits: CodecLimits) -> Result<(), CodecError> {
    let mut targets: Vec<&[PathSegment]> = Vec::new();
    for transform in transforms {
        match transform {
            Transform::NamesVector { path } => {
                validate_path(path, limits)?;
                targets.push(path);
            }
            Transform::SharedDefaults { path, defaults } => {
                validate_path(path, limits)?;
                validate_defaults(defaults)?;
                scan_value(&Value::Object(defaults.clone()), limits)?;
                targets.push(path);
            }
            Transform::SharedMembers { paths, members } => {
                if paths.len() < 2 {
                    return Err(CodecError::Malformed(
                        "shared_members requires at least two paths".to_owned(),
                    ));
                }
                if paths.len() > limits.max_paths_per_transform {
                    return Err(CodecError::BudgetExceeded("paths per transform"));
                }
                if members.len() < 2 {
                    return Err(CodecError::Malformed(
                        "shared_members requires a non-trivial roster".to_owned(),
                    ));
                }
                for path in paths {
                    validate_path(path, limits)?;
                    if !is_member_roster_path(path) {
                        return Err(CodecError::Malformed(
                            "shared_members paths must end in members".to_owned(),
                        ));
                    }
                    targets.push(path);
                }
            }
        }
    }
    for index in 0..targets.len() {
        for other in (index + 1)..targets.len() {
            if paths_overlap(targets[index], targets[other]) {
                return Err(CodecError::Malformed(
                    "duplicate or overlapping transform paths".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_path(path: &[PathSegment], limits: CodecLimits) -> Result<(), CodecError> {
    if path.is_empty() {
        return Err(CodecError::Malformed(
            "root transforms are not supported".to_owned(),
        ));
    }
    if path.len() > limits.max_path_segments {
        return Err(CodecError::BudgetExceeded("path depth"));
    }
    if !path_is_safe(path) {
        return Err(CodecError::Malformed(
            "prototype-like transform path key".to_owned(),
        ));
    }
    Ok(())
}

fn path_is_safe(path: &[PathSegment]) -> bool {
    path.iter().all(|segment| match segment {
        PathSegment::Key(key) => !matches!(key.as_str(), "__proto__" | "prototype" | "constructor"),
        PathSegment::Index(_) => true,
    })
}

fn paths_overlap(left: &[PathSegment], right: &[PathSegment]) -> bool {
    let shared = left.len().min(right.len());
    left[..shared] == right[..shared]
}

fn validate_defaults(defaults: &Map<String, Value>) -> Result<(), CodecError> {
    if defaults.is_empty() {
        return Err(CodecError::Malformed(
            "shared_defaults has no defaults".to_owned(),
        ));
    }
    for (key, value) in defaults {
        let valid = match key.as_str() {
            MODE_FIELD => matches!(value, Value::String(mode) if mode == "CONSTANT"),
            EVALUATION_FIELD => {
                let expected = serde_json::json!({"available": true});
                exact_value_eq(value, &expected)
            }
            _ => false,
        };
        if !valid {
            return Err(CodecError::Malformed(format!(
                "unsupported or negative shared default {key:?}"
            )));
        }
    }
    Ok(())
}

fn decode_names(data: &mut Value, path: &[PathSegment]) -> Result<(), CodecError> {
    let target = get_at_mut(data, path).ok_or_else(|| malformed_path(path, "not found"))?;
    let array = target
        .as_array_mut()
        .ok_or_else(|| malformed_path(path, "names target is not an array"))?;
    for value in array.iter_mut() {
        let Value::String(name) = std::mem::take(value) else {
            return Err(malformed_path(path, "names vector contains a non-string"));
        };
        let mut row = Map::new();
        row.insert(NAME_FIELD.to_owned(), Value::String(name));
        *value = Value::Object(row);
    }
    Ok(())
}

fn decode_defaults(
    data: &mut Value,
    path: &[PathSegment],
    defaults: Map<String, Value>,
) -> Result<(), CodecError> {
    let target = get_at_mut(data, path).ok_or_else(|| malformed_path(path, "not found"))?;
    let array = target
        .as_array_mut()
        .ok_or_else(|| malformed_path(path, "defaults target is not an array"))?;
    for row in array {
        let object = row
            .as_object_mut()
            .ok_or_else(|| malformed_path(path, "defaults target contains a non-object"))?;
        for key in defaults.keys() {
            if object.contains_key(key) {
                return Err(malformed_path(
                    path,
                    "default would overwrite an existing field",
                ));
            }
        }
        for (key, value) in &defaults {
            object.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn insert_members(
    data: &mut Value,
    path: &[PathSegment],
    members: &[String],
) -> Result<(), CodecError> {
    let Some((last, parent_path)) = path.split_last() else {
        return Err(malformed_path(path, "empty path"));
    };
    let PathSegment::Key(key) = last else {
        return Err(malformed_path(path, "members path does not end in a key"));
    };
    if key != MEMBERS_FIELD {
        return Err(malformed_path(path, "members path has wrong final key"));
    }
    let parent = get_at_mut(data, parent_path)
        .ok_or_else(|| malformed_path(path, "members parent not found"))?;
    let object = parent
        .as_object_mut()
        .ok_or_else(|| malformed_path(path, "members parent is not an object"))?;
    if object.contains_key(key) {
        return Err(malformed_path(
            path,
            "members transform would overwrite an existing field",
        ));
    }
    object.insert(
        key.clone(),
        Value::Array(members.iter().cloned().map(Value::String).collect()),
    );
    Ok(())
}

fn get_at<'a>(mut value: &'a Value, path: &[PathSegment]) -> Option<&'a Value> {
    for segment in path {
        value = match segment {
            PathSegment::Key(key) => value.as_object()?.get(key)?,
            PathSegment::Index(index) => value.as_array()?.get(*index)?,
        };
    }
    Some(value)
}

fn get_at_mut<'a>(mut value: &'a mut Value, path: &[PathSegment]) -> Option<&'a mut Value> {
    for segment in path {
        value = match segment {
            PathSegment::Key(key) => value.as_object_mut()?.get_mut(key)?,
            PathSegment::Index(index) => value.as_array_mut()?.get_mut(*index)?,
        };
    }
    Some(value)
}

fn remove_at(value: &mut Value, path: &[PathSegment]) -> Option<Value> {
    let (last, parent_path) = path.split_last()?;
    let parent = get_at_mut(value, parent_path)?;
    match last {
        PathSegment::Key(key) => parent.as_object_mut()?.remove(key),
        PathSegment::Index(index) => {
            let array = parent.as_array_mut()?;
            (*index < array.len()).then(|| array.remove(*index))
        }
    }
}

fn malformed_path(path: &[PathSegment], message: &str) -> CodecError {
    let rendered = path.iter().map(ToString::to_string).collect::<String>();
    CodecError::Malformed(format!("path ${rendered}: {message}"))
}
