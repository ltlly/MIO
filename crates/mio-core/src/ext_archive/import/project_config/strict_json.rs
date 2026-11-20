use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub(super) enum StrictValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Array(Vec<StrictValue>),
    Object(BTreeMap<String, StrictValue>),
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct StrictVisitor;

        impl<'de> serde::de::Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("不含重复键的合法 JSON 值")
            }

            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::Null)
            }

            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::Bool(v))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::Int(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::Uint(v))
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::Float(v))
            }

            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::String(v.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue::String(v))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(item) = seq.next_element()? {
                    values.push(item);
                }
                Ok(StrictValue::Array(values))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, StrictValue>()? {
                    if entries.insert(key.clone(), value).is_some() {
                        return Err(serde::de::Error::custom(format!("JSON 包含重复键 `{key}`")));
                    }
                }
                Ok(StrictValue::Object(entries))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

pub(super) fn parse_strict_object(text: &str) -> Result<BTreeMap<String, StrictValue>> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = StrictValue::deserialize(&mut decoder)?;
    decoder.end().context("JSON 存在尾随内容")?;
    let StrictValue::Object(map) = value else {
        bail!("顶层必须是 JSON 对象");
    };
    Ok(map)
}

pub(super) fn to_serde_value(value: &StrictValue) -> serde_json::Value {
    match value {
        StrictValue::Null => serde_json::Value::Null,
        StrictValue::Bool(v) => serde_json::Value::Bool(*v),
        StrictValue::Int(v) => serde_json::Value::Number((*v).into()),
        StrictValue::Uint(v) => serde_json::Value::Number((*v).into()),
        StrictValue::Float(v) => serde_json::Number::from_f64(*v)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        StrictValue::String(v) => serde_json::Value::String(v.clone()),
        StrictValue::Array(items) => {
            serde_json::Value::Array(items.iter().map(to_serde_value).collect())
        }
        StrictValue::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_serde_value(value)))
                .collect(),
        ),
    }
}
