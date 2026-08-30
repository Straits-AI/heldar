//! Emitting a result in exactly one of two shapes (#122).
//!
//! Human output is for reading; `--output=json` is for scripts and is a stable contract. They are
//! produced from the SAME value, so the human rendering cannot show something the JSON does not
//! contain — a table that says more than the machine output is how an automation ends up blind to
//! something an operator can see.

use serde::Serialize;

/// Print `value` as JSON, or as the human rendering of it.
pub fn emit<T: Serialize>(value: &T, json: bool, human: impl FnOnce(&serde_json::Value) -> String) {
    let v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    if json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        println!("{}", human(&v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The human rendering is derived from the serialized value, so it cannot invent a field the
    /// JSON lacks — the renderer is only handed the JSON.
    #[test]
    fn the_human_rendering_can_only_see_what_the_json_holds() {
        #[derive(Serialize)]
        struct S {
            a: u8,
        }
        let mut seen = String::new();
        emit(&S { a: 1 }, false, |v| {
            seen = v.to_string();
            String::new()
        });
        assert_eq!(seen, r#"{"a":1}"#);
    }
}
