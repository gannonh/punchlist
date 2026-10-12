// Copied from kata-symphony apps/symphony/src/codex/token_accounting.rs at e7c160f5; trimmed: kept the token-usage extraction and the per-event delta; dropped the total-token dimension and rate-limit extraction, which the runner does not report.
//! Token accounting for Codex sessions: absolute token totals from event payloads, turned
//! into per-event deltas.

use serde_json::Value;

/// Snapshot of the last-reported absolute token totals.
///
/// Stored between events so deltas can be computed correctly.
#[derive(Debug, Clone, Default)]
pub struct TokenState {
    /// Last-seen absolute input token total (0 if never reported).
    pub last_input: u64,
    /// Last-seen absolute output token total (0 if never reported).
    pub last_output: u64,
}

/// Per-event incremental token consumption.
///
/// All deltas are guaranteed non-negative (zero when the next reported total
/// is less than the previous).
#[derive(Debug, Clone, Default)]
pub struct TokenDelta {
    /// Incremental input tokens consumed since last event.
    pub input_tokens: u64,
    /// Incremental output tokens consumed since last event.
    pub output_tokens: u64,
    /// Absolute input-token total reported in this event (for updating state).
    pub input_reported: u64,
    /// Absolute output-token total reported in this event (for updating state).
    pub output_reported: u64,
}

/// Extract token delta from an event payload relative to `last_state`.
///
/// Searches the payload at all known nested paths for an absolute token usage
/// map, computes the increment over `last_state`, and returns the delta plus
/// new absolute totals for the caller to persist.
///
/// Delta is 0 (not negative) when `next_total < last_reported`.
pub fn extract_token_delta(last_state: &TokenState, event_payload: &Value) -> TokenDelta {
    let usage = extract_token_usage(event_payload);

    let (input_delta, input_reported) =
        compute_delta(get_token_usage_input(&usage), last_state.last_input);
    let (output_delta, output_reported) =
        compute_delta(get_token_usage_output(&usage), last_state.last_output);

    TokenDelta {
        input_tokens: input_delta,
        output_tokens: output_delta,
        input_reported,
        output_reported,
    }
}

/// Extract a token-usage map from the event payload.
///
/// Tries the following lookup strategies in order:
/// 1. Absolute paths (nested usage map with known structure)
/// 2. `turn/completed` direct `usage` field
/// 3. Top-level payload as usage map
///
/// Returns the first matching map, or an empty object if none found.
fn extract_token_usage(payload: &Value) -> Value {
    // Strategy 1: absolute paths
    let candidates: &[&[&str]] = &[
        &["params", "msg", "payload", "info", "total_token_usage"],
        &["params", "msg", "info", "total_token_usage"],
        &["params", "tokenUsage", "total"],
        &["tokenUsage", "total"],
    ];

    for path in candidates {
        if let Some(v) = value_at_path(payload, path)
            && is_integer_token_map(v)
        {
            return v.clone();
        }
    }

    // Strategy 2: turn/completed usage field
    if payload.get("method").and_then(|m| m.as_str()) == Some("turn/completed") {
        // Check direct "usage" field and params.usage
        let usage_candidates = [
            payload.get("usage"),
            payload.get("params").and_then(|p| p.get("usage")),
        ];
        for u in usage_candidates.into_iter().flatten() {
            if is_integer_token_map(u) {
                return u.clone();
            }
        }
    }

    // Strategy 3: check top-level fields
    if is_integer_token_map(payload) {
        return payload.clone();
    }

    // Check usage field at top level
    if let Some(u) = payload.get("usage")
        && is_integer_token_map(u)
    {
        return u.clone();
    }

    Value::Object(serde_json::Map::new())
}

/// Return `true` if `value` is an object containing at least one integer token field.
fn is_integer_token_map(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };

    let token_fields = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "prompt_tokens",
        "completion_tokens",
        "inputTokens",
        "outputTokens",
        "totalTokens",
        "promptTokens",
        "completionTokens",
    ];

    token_fields
        .iter()
        .any(|field| obj.get(*field).and_then(integer_like).is_some())
}

fn get_token_usage_input(usage: &Value) -> Option<u64> {
    let fields = [
        "input_tokens",
        "prompt_tokens",
        "inputTokens",
        "promptTokens",
    ];
    fields
        .iter()
        .find_map(|f| usage.get(f).and_then(integer_like))
}

fn get_token_usage_output(usage: &Value) -> Option<u64> {
    let fields = [
        "output_tokens",
        "completion_tokens",
        "outputTokens",
        "completionTokens",
    ];
    fields
        .iter()
        .find_map(|f| usage.get(f).and_then(integer_like))
}

/// Compute `(delta, reported)` for one token dimension.
///
/// Returns `(delta=0, reported=prev)` if `next_total` is `None`.
/// Returns `(delta=0, reported=next)` if `next < prev` (never negative).
fn compute_delta(next_total: Option<u64>, prev_reported: u64) -> (u64, u64) {
    match next_total {
        None => (0, prev_reported),
        Some(next) => (next.saturating_sub(prev_reported), next),
    }
}

/// Traverse `payload` along `path`, returning the value at the endpoint.
fn value_at_path<'a>(payload: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = payload;
    for key in path {
        current = current.get(key)?;
    }
    Some(current)
}

/// Return `Some(u64)` if `value` is a non-negative integer or a parseable
/// string, `None` otherwise.
fn integer_like(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_payload_returns_zero_delta() {
        let state = TokenState::default();
        let delta = extract_token_delta(&state, &json!({}));
        assert_eq!(delta.input_tokens, 0);
        assert_eq!(delta.output_tokens, 0);
        assert_eq!(delta.input_reported, 0);
    }

    #[test]
    fn absolute_path_extraction() {
        let state = TokenState::default();
        let payload = json!({
            "params": {
                "tokenUsage": {
                    "total": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "total_tokens": 150
                    }
                }
            }
        });
        let delta = extract_token_delta(&state, &payload);
        assert_eq!(delta.input_tokens, 100);
        assert_eq!(delta.output_tokens, 50);
        assert_eq!(delta.output_reported, 50);
    }

    #[test]
    fn delta_is_zero_on_decrease() {
        let state = TokenState {
            last_input: 200,
            ..Default::default()
        };
        let payload = json!({
            "params": {
                "tokenUsage": {
                    "total": { "inputTokens": 100 }
                }
            }
        });
        let delta = extract_token_delta(&state, &payload);
        assert_eq!(delta.input_tokens, 0, "delta must be 0 when next < prev");
        assert_eq!(
            delta.input_reported, 100,
            "reported should update even on decrease"
        );
    }

    #[test]
    fn incremental_delta_computation() {
        let state = TokenState {
            last_input: 100,
            ..Default::default()
        };
        let payload = json!({
            "tokenUsage": {
                "total": { "inputTokens": 250 }
            }
        });
        let delta = extract_token_delta(&state, &payload);
        assert_eq!(delta.input_tokens, 150);
        assert_eq!(delta.input_reported, 250);
    }
}
