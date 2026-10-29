//! Reading HTML forms into Rust types with serde, for what browsers send that
//! serde does not expect on its own:
//!
//! ```rust,ignore
//! #[derive(Deserialize)]
//! pub struct ProductInput {
//!     pub name: String,
//!     /// An empty field is `None`, not an error.
//!     #[serde(default, deserialize_with = "custard::form::optional")]
//!     pub price: Option<i64>,
//!     /// A ticked box is sent as `on`, an unticked one not at all.
//!     #[serde(default, deserialize_with = "custard::form::checkbox")]
//!     pub in_stock: bool,
//! }
//! ```

use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Deserializer, de::Error as _};

/// A checkbox: `true` when the field was sent at all, as browsers leave
/// unticked boxes out. Pair it with `#[serde(default)]`.
pub fn checkbox<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Option::<String>::deserialize(deserializer).map(|value| value.is_some())
}

/// An optional field: `None` when it is missing or blank, otherwise its
/// trimmed text parsed as `T`. Pair it with `#[serde(default)]`.
pub fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    match Option::<String>::deserialize(deserializer)?
        .as_deref()
        .map(str::trim)
    {
        None | Some("") => Ok(None),
        Some(text) => text.parse().map(Some).map_err(D::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Input {
        #[serde(default, deserialize_with = "super::optional")]
        price: Option<i64>,
        #[serde(default, deserialize_with = "super::optional")]
        note: Option<String>,
        #[serde(default, deserialize_with = "super::checkbox")]
        done: bool,
    }

    fn parse(form: &str) -> Result<Input, String> {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect();
        let value = serde_json::Value::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
        );
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    #[test]
    fn reads_what_browsers_send() {
        assert_eq!(
            parse("price=12&note=hi&done=on").unwrap(),
            Input {
                price: Some(12),
                note: Some("hi".into()),
                done: true
            }
        );
        assert_eq!(
            parse("price=&note=+").unwrap(),
            Input {
                price: None,
                note: None,
                done: false
            }
        );
        assert_eq!(
            parse("").unwrap(),
            Input {
                price: None,
                note: None,
                done: false
            }
        );
        assert!(parse("price=twelve").is_err());
    }
}
