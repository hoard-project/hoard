#![allow(dead_code)]
//! Environment variable expansion for config strings.
//!
//! Supports `${ENV_VAR}` placeholders.  Missing variables are
//! silently left as literals (the caller should validate).
#![deny(unsafe_code)]

pub fn expand_env(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // skip '{'
            let mut var = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var.push(ch);
            }
            if let Ok(val) = std::env::var(&var) {
                result.push_str(&val);
            } else {
                // Preserve missing variable as literal
                result.push_str("${");
                result.push_str(&var);
                result.push('}');
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Expand env vars in all string fields of a TOML value, recursively.
pub fn expand_table(table: &mut toml::Table) {
    for (_key, value) in table.iter_mut() {
        match value {
            toml::Value::String(s) => {
                *s = expand_env(s);
            }
            toml::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    if let toml::Value::String(s) = v {
                        *s = expand_env(s);
                    }
                }
            }
            toml::Value::Table(inner) => expand_table(inner),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_expansion() {
        std::env::set_var("HOARD_TEST_FOO", "bar");
        assert_eq!(
            expand_env("prefix_${HOARD_TEST_FOO}_suffix"),
            "prefix_bar_suffix"
        );
    }

    #[test]
    fn missing_var_preserved() {
        assert_eq!(expand_env("${NO_SUCH_VAR_XYZ}"), "${NO_SUCH_VAR_XYZ}");
    }

    #[test]
    fn multiple_vars() {
        std::env::set_var("A", "1");
        std::env::set_var("B", "2");
        assert_eq!(expand_env("${A}_${B}"), "1_2");
    }
}
