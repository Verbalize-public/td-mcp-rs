//! Lenient numeric wire types: a JSON number *or* a decimal numeric string.
//!
//! MCP clients stringify numeric ids/limits inconsistently (e.g. `"pid":
//! "4988"`); every wire-facing unsigned integer param therefore accepts both
//! shapes at the parse boundary. Serialization always emits a plain number,
//! so the daemon→bridge hop (and Python) never observes strings. The string
//! form also carries u64 precision past 2^53, which JSON numbers cannot.
//!
//! Acceptance policy (single source): JSON integers within range, integral
//! JSON floats within range (e.g. `4988.0`), and decimal numeric strings
//! after trimming (e.g. `"4988"`, `" 4988 "`). Rejected: negatives, overflow,
//! non-integers, and anything else `str::parse` refuses (`"0x10"`, `""`,
//! `"1.5"`, `"+ "`). Schema-wise each type advertises
//! `anyOf: [integer, string]` so schema-strict clients may send either.

use std::fmt;

use serde::Serialize;

/// Generates the lenient `Deserialize` + `JsonSchema` impls for a tuple
/// newtype over an unsigned integer. Shared by [`Pid`](crate::Pid) (ids.rs)
/// and the `LenientU32` / `LenientU64` wire types below.
///
/// `$format` is the JSON Schema `format` hint, `$expecting` the visitor's
/// `expecting()` text, `$doc` the schema description (keep it in sync with
/// the type's rustdoc).
macro_rules! impl_lenient_uint {
    ($ty:ident, $inner:ty, $format:literal, $expecting:literal, $doc:literal) => {
        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct V;

                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = $ty;

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str($expecting)
                    }

                    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        <$inner>::try_from(v)
                            .map($ty)
                            .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(v), &self))
                    }

                    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        <$inner>::try_from(v)
                            .map($ty)
                            .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(v), &self))
                    }

                    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        if v.is_finite() && v.fract() == 0.0 && v >= 0.0 {
                            if let Ok(n) = <$inner>::try_from(v as u128) {
                                return Ok($ty(n));
                            }
                        }
                        Err(E::invalid_value(serde::de::Unexpected::Float(v), &self))
                    }

                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        crate::numeric::parse_decimal(v)
                            .map($ty)
                            .ok_or_else(|| E::invalid_type(serde::de::Unexpected::Str(v), &self))
                    }
                }

                deserializer.deserialize_any(V)
            }
        }

        impl schemars::JsonSchema for $ty {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($ty))
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "description": $doc,
                    "anyOf": [
                        { "type": "integer", "format": $format, "minimum": 0 },
                        { "type": "string", "pattern": "^\\s*[0-9]+\\s*$" }
                    ]
                })
            }
        }
    };
}

pub(crate) use impl_lenient_uint;

/// Single string-acceptance policy: trim, then decimal parse.
pub(crate) fn parse_decimal<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim().parse::<T>().ok()
}

/// Unsigned 32-bit integer; accepts a JSON number or a decimal numeric string.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LenientU32(pub u32);

impl_lenient_uint!(
    LenientU32,
    u32,
    "uint32",
    "a u32 or a decimal numeric string",
    "Unsigned 32-bit integer; accepts a JSON number or a decimal numeric string."
);

impl LenientU32 {
    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for LenientU32 {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl fmt::Display for LenientU32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unsigned 64-bit integer; accepts a JSON number or a decimal numeric string.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LenientU64(pub u64);

impl_lenient_uint!(
    LenientU64,
    u64,
    "uint64",
    "a u64 or a decimal numeric string",
    "Unsigned 64-bit integer; accepts a JSON number or a decimal numeric string."
);

impl LenientU64 {
    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for LenientU64 {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for LenientU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::{LenientU32, LenientU64};
    use serde_json::{from_value, json, to_value};

    #[test]
    fn lenient_u32_accepts_number_string_and_integral_float() {
        for ok in [
            json!(7),
            json!("7"),
            json!(" 7 "),
            json!("0007"),
            json!(7.0),
        ] {
            let parsed: LenientU32 = from_value(ok.clone()).unwrap_or_else(|e| panic!("{ok}: {e}"));
            assert_eq!(parsed.get(), 7, "{ok}");
        }
        // u64::MAX fits the 64-bit variant.
        let max: LenientU64 = from_value(json!("18446744073709551615")).unwrap();
        assert_eq!(max.get(), u64::MAX);
    }

    #[test]
    fn lenient_u64_string_keeps_precision_past_two_pow_53() {
        let v = 9_007_199_254_740_993_u64; // 2^53 + 1
        let parsed: LenientU64 = from_value(json!(v.to_string())).unwrap();
        assert_eq!(parsed.get(), v);
    }

    #[test]
    fn rejects_non_numeric_shapes() {
        for bad in [
            json!(-1),
            json!(i64::MIN),
            json!(1.5),
            json!("abc"),
            json!(""),
            json!("  "),
            json!("0x10"),
            json!("1.5"),
            json!(null),
            json!(true),
            json!([7]),
        ] {
            assert!(
                from_value::<LenientU32>(bad.clone()).is_err(),
                "LenientU32 accepted {bad}"
            );
            assert!(
                from_value::<LenientU64>(bad.clone()).is_err(),
                "LenientU64 accepted {bad}"
            );
        }
        // Range overflows are width-specific.
        assert!(from_value::<LenientU32>(json!(4_294_967_296_u64)).is_err());
        assert!(from_value::<LenientU64>(json!(4_294_967_296_u64)).is_ok());
    }

    #[test]
    fn serialize_emits_plain_number() {
        assert_eq!(to_value(LenientU32(4988)).unwrap(), json!(4988));
        assert_eq!(to_value(LenientU64(1)).unwrap(), json!(1));
    }

    #[test]
    fn schema_advertises_integer_or_string() {
        let s = serde_json::to_value(schemars::schema_for!(LenientU32)).unwrap();
        let any_of = s
            .get("anyOf")
            .and_then(serde_json::Value::as_array)
            .expect("anyOf present");
        assert_eq!(any_of.len(), 2);
        assert_eq!(any_of[0]["type"], "integer");
        assert_eq!(any_of[0]["format"], "uint32");
        assert_eq!(any_of[1]["type"], "string");
    }
}
