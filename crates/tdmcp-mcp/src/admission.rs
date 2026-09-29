//! What the caller did and did not get, counted in four separate quantities.
//!
//! An image passes through four states, and the honest reporting requirement
//! is that they are never collapsed into one word. "attached" can mean any of
//! them, and a caller who reads it as the last one is wrong three times out of
//! four:
//!
//! | quantity                  | observed by | means                                              |
//! | ------------------------- | ----------- | -------------------------------------------------- |
//! | `generated`               | bridge      | TD produced these pixels                           |
//! | `emitted`                 | server      | the bytes are in this result                        |
//! | `admitted`                | server      | the bytes are in `content[]` as an attachment       |
//! | `includedInModelInput`    | **nobody**  | the client's model actually saw them                |
//!
//! The fourth is the one that matters most and the one this server cannot
//! observe. Emitting an image block proves the server sent bytes; it proves
//! nothing about what the client's model was given. A client may drop the
//! attachment, downscale it before the vision call, or never invoke a model at
//! all — and the server has no signal for any of that. So it is reported as
//! `unavailable` rather than assumed, and the distinction is the whole reason
//! this module exists: an evaluation that counts "the server sent an image" as
//! evidence of visual inspection is measuring its own transport.
//!
//! What the server *can* decide is whether the bytes go into `content[]` as an
//! attachment, bounded by [`AdmissionPolicy::max_image_bytes`]. The three sites
//! that create image content blocks all route through [`admit_image`], so the
//! policy has exactly one implementation and an image cannot be admitted by one
//! path and withheld by another.

use serde_json::{json, Value};

/// The configured bound on encoded image bytes admitted into `content[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionPolicy {
    /// Bytes of encoded image content admitted into an MCP result's
    /// `content[]`. An image above this is still emitted in
    /// `structuredContent` and still reported — it is withheld from the
    /// attachment list, never dropped.
    pub max_image_bytes: usize,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: tdmcp_config::DEFAULT_ADMISSION_MAX_IMAGE_BYTES,
        }
    }
}

/// The verdict for one image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The bytes are in `content[]` as an attachment.
    Admitted,
    /// The bytes are above the bound and stay in `structuredContent` only.
    Withheld,
}

impl Admission {
    /// The name used in the record's `admitted` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Withheld => "withheld",
        }
    }
}

/// Decide whether one encoded image enters the attachment list.
///
/// `encoded_bytes` is the size of the image itself, not of its base64 form. The
/// bound is expressed in encoded bytes because that is what the caller can
/// reason about — `bytes` in the capture response — and comparing against the
/// base64 length would make a 6 MiB image report as 8 MiB, which is a number
/// the caller cannot check against anything it was told.
#[must_use]
pub fn admit_image(policy: AdmissionPolicy, encoded_bytes: usize) -> Admission {
    if encoded_bytes <= policy.max_image_bytes {
        Admission::Admitted
    } else {
        Admission::Withheld
    }
}

/// Build the `admission` block for a result that carried `images` images.
///
/// `generated_bytes` is the sum of the encoded sizes of the images the bridge
/// produced. It is reported separately from what was admitted so a caller can
/// see the gap: an admitted total below the generated total means something
/// was withheld, and without both numbers the difference is invisible.
///
/// This block goes into `structuredContent`, not `content[]`. A text-rendering
/// client is exactly the client that needs to be told the image was withheld,
/// and it is the one that cannot see an attachment list.
#[must_use]
pub fn admission_record(
    policy: AdmissionPolicy,
    images: usize,
    admitted: usize,
    generated_bytes: u64,
    withheld_bytes: u64,
) -> Value {
    json!({
        "generated": {
            "images": images,
            "bytes": generated_bytes,
            "observedBy": "bridge",
        },
        "emitted": {
            "inStructuredContent": true,
            "note": "withheld images keep their bytes in structuredContent.imageBase64",
        },
        "admitted": {
            "images": admitted,
            "verdict": if images == 0 { "not_applicable" } else if admitted == images {
                "admitted"
            } else if admitted == 0 {
                "withheld"
            } else {
                "partial"
            },
            "maxImageBytes": policy.max_image_bytes,
            "withheldBytes": withheld_bytes,
            "observedBy": "server",
        },
        "includedInModelInput": {
            "state": "unavailable",
            "observedBy": "none",
            "note": "this server cannot observe what a client's model was given; an \
                     attachment in content[] proves the bytes were sent, not that they \
                     were seen. A client may drop, downscale or ignore the attachment.",
        },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn the_bound_compares_encoded_bytes_not_base64() {
        // 7 MiB encoded. Its base64 form is ~9.33 MiB, so a policy that
        // compared the WIRE form would refuse this image while advertising an
        // 8 MiB bound — a caller holding 7 MiB of PNG and told "8 MiB limit"
        // could not reconcile the two numbers, and a capture that fits the
        // documented limit would be silently withheld.
        let seven_mib: usize = 7 * 1024 * 1024;
        let base64 = seven_mib.div_ceil(3) * 4;
        assert!(base64 > 8 * 1024 * 1024, "the two forms must differ here");
        let policy = AdmissionPolicy {
            max_image_bytes: 8 * 1024 * 1024,
        };
        assert_eq!(admit_image(policy, seven_mib), Admission::Admitted);
        // The same image, measured the way a wire-comparing policy would.
        assert_eq!(admit_image(policy, base64), Admission::Withheld);
    }

    #[test]
    fn the_bound_is_inclusive_so_a_configured_limit_is_the_last_admitted_size() {
        let policy = AdmissionPolicy {
            max_image_bytes: 100,
        };
        assert_eq!(admit_image(policy, 100), Admission::Admitted);
        assert_eq!(admit_image(policy, 101), Admission::Withheld);
    }

    #[test]
    fn a_zero_budget_withholds_rather_than_admits_everything() {
        // The server refuses to construct this state (see `with_admission`),
        // but the function must still be total: a zero bound cannot mean
        // "unlimited", which is how a bare `bytes > 0` comparison reads.
        let policy = AdmissionPolicy { max_image_bytes: 0 };
        assert_eq!(admit_image(policy, 1), Admission::Withheld);
    }

    #[test]
    fn the_model_input_state_is_never_assumed() {
        let block = admission_record(AdmissionPolicy::default(), 1, 1, 100, 0);
        let state = &block["includedInModelInput"];
        assert_eq!(state["state"], "unavailable");
        assert_eq!(state["observedBy"], "none");
        // If this ever reads "yes" the whole module has stopped being honest.
        assert_ne!(state["state"], "confirmed");
    }

    #[test]
    fn a_withheld_image_reports_the_generated_bytes_it_cost() {
        let block = admission_record(AdmissionPolicy::default(), 2, 1, 9000, 5000);
        assert_eq!(block["generated"]["images"], 2);
        assert_eq!(block["generated"]["bytes"], 9000);
        assert_eq!(block["admitted"]["images"], 1);
        assert_eq!(block["admitted"]["verdict"], "partial");
        assert_eq!(block["admitted"]["withheldBytes"], 5000);
    }

    #[test]
    fn a_result_with_no_images_is_not_applicable_not_passed() {
        let block = admission_record(AdmissionPolicy::default(), 0, 0, 0, 0);
        assert_eq!(block["admitted"]["verdict"], "not_applicable");
        // "admitted" for zero images would be a green light for a check that
        // never ran.
        assert_ne!(block["admitted"]["verdict"], "admitted");
    }

    #[test]
    fn a_fully_withheld_result_says_withheld_not_partial() {
        let block = admission_record(AdmissionPolicy::default(), 1, 0, 9000, 9000);
        assert_eq!(block["admitted"]["verdict"], "withheld");
    }

    /// The default bound is a claim about real capture sizes, so it is checked
    /// against real capture sizes rather than against the author's memory of
    /// them. The sample is [`include_str!`]ed rather than copied so a later
    /// edit to the measurement cannot leave this test asserting a stale number.
    ///
    /// What this proves and what it does not: every recorded observation is
    /// admitted, so the bound does not withhold the captures anyone has
    /// actually taken. It does not prove the bound is right — that needs a
    /// distribution, and the sample is nine PNGs of one noise source. The
    /// `max_observed` assertion is the honest half: it fails loudly if a future
    /// capture is large enough to change what the bound should be.
    #[test]
    fn the_default_bound_admits_every_capture_we_have_measured() {
        let sample: serde_json::Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/admission/live-capture-bytes.json"
        ))
        .expect("the admission measurement fixture is readable JSON");
        let observations = sample["observations"]
            .as_array()
            .expect("observations is a list");
        assert!(
            observations.len() >= 5,
            "a sample of {} captures is too thin to justify a default",
            observations.len()
        );

        let default = AdmissionPolicy::default();
        let mut largest = 0usize;
        for row in observations {
            let id = row["id"].as_str().expect("every observation is named");
            let bytes = row["encodedBytes"]
                .as_u64()
                .expect("{id}: encodedBytes is a number") as usize;
            largest = largest.max(bytes);
            assert_eq!(
                admit_image(default, bytes),
                Admission::Admitted,
                "{id} ({bytes} bytes) is withheld by the default bound, so the \
                 bound is now tighter than a capture anyone has taken"
            );
        }
        // The measurement is a floor on the evidence, not a ceiling on the
        // sizes the product can produce; this only fails if the sample changes.
        assert_eq!(largest, 440_234, "the largest measured capture changed");
    }
}
