//! Python `str()` / `repr()` semantics for JSON values, needed because
//! sounds.py normalises every sounds.json entry through `str(x)`
//! (`str(k): [str(x) for x in v] if isinstance(v, list) else str(v)`).
//! JSON `true` becomes `"True"`, `null` becomes `"None"`, and `str()` of a
//! list/dict renders its elements with `repr()` (`str(["a"]) == "['a']"`),
//! all of which differ from Rust's `Display` conventions.

use serde_json::Value;

/// Python's `str(v)` for a decoded JSON value.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                n.as_f64().map_or_else(|| n.to_string(), py_float_str)
            }
        }
        // str(list) and str(dict) print their elements with repr(), not str().
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Python's `repr(v)` for a decoded JSON value - identical to `str()` for
/// everything except strings, which gain quotes and escapes.
pub(crate) fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => py_str_repr(s),
        other => py_str(other),
    }
}

/// Python's `repr()` of a string: single quotes unless the text contains a
/// single quote and no double quote; `\n`/`\r`/`\t`/backslash/quote escaped,
/// other control characters as `\xHH`.
fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `str(float)`: shortest round-trip digits, a `.0` on integral
/// values, fixed notation for decimal exponents -4..=15 and exponential
/// (`1e+16`, `1e-05`) outside that range.
fn py_float_str(x: f64) -> String {
    let negative = x.is_sign_negative();
    let abs = x.abs();
    if abs == 0.0 {
        return if negative {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    // Rust's `{:e}` yields the shortest round-trip digits as `d.ddde<exp>`,
    // which matches the digits/exponent pair CPython's repr works from.
    let sci = format!("{abs:e}");
    let (mantissa, exponent) = sci.split_once('e').expect("LowerExp always emits 'e'");
    let exponent: i32 = exponent.parse().expect("LowerExp exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if negative { "-" } else { "" };
    if !(-4..=15).contains(&exponent) {
        // Exponential form: `1e+16`, `1.234e-05` (sign always, 2-digit min).
        let mut out = format!("{sign}{}", &digits[..1]);
        let rest = &digits[1..];
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        ));
        out
    } else if exponent < 0 {
        // Fixed form below 1: `0.0001`.
        format!(
            "{sign}0.{}{}",
            "0".repeat(exponent.unsigned_abs() as usize - 1),
            digits
        )
    } else {
        let point = exponent as usize + 1;
        if point >= digits.len() {
            // Integral: `1000000000000000.0`
            format!("{sign}{digits}{}.0", "0".repeat(point - digits.len()))
        } else {
            // Mixed: `1.5`
            format!("{sign}{}.{}", &digits[..point], &digits[point..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_str_renders_python_scalars() {
        assert_eq!(py_str(&Value::Bool(true)), "True");
        assert_eq!(py_str(&Value::Bool(false)), "False");
        assert_eq!(py_str(&Value::Null), "None");
        assert_eq!(py_str(&Value::from(5)), "5");
        assert_eq!(py_str(&Value::from(-5)), "-5");
        assert_eq!(py_str(&serde_json::json!(1.0)), "1.0");
        assert_eq!(py_str(&serde_json::json!(1.5)), "1.5");
        assert_eq!(py_str(&serde_json::json!(0.0001)), "0.0001");
        assert_eq!(py_str(&serde_json::json!(1e-5)), "1e-05");
        assert_eq!(py_str(&serde_json::json!(1e16)), "1e+16");
        assert_eq!(py_str(&serde_json::json!(1e15)), "1000000000000000.0");
    }

    #[test]
    fn py_str_repr_quotes_strings_inside_containers() {
        assert_eq!(py_str(&Value::from("yawn")), "yawn");
        assert_eq!(py_repr(&Value::from("yawn")), "'yawn'");
        assert_eq!(py_str(&serde_json::json!(["a"])), "['a']");
    }
}
