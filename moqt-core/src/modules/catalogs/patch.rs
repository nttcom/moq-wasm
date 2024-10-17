use std::collections::HashMap;

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

use crate::catalogs::common_catalog::{CatalogObject, CommonTrackFields, SelectionParams, Track};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op")]
pub enum JsonPatchOp {
    #[serde(rename = "add")]
    Add(Add),
    #[serde(rename = "remove")]
    Remove(Remove),
    #[serde(rename = "replace")]
    Replace(Replace),
    #[serde(rename = "move")]
    Move(Move),
    #[serde(rename = "copy")]
    Copy(Copy),
    #[serde(rename = "test")]
    Test(Test),
}

fn deserialize_slash_separated<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Ok(s.split('/').map(String::from).collect())
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PatchValue {
    String(String),
    Track(Track),
    CatalogObject(CatalogObject),
    CommonTrackFields(CommonTrackFields),
    SelectionParams(SelectionParams),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Add {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub path: Vec<String>,
    pub value: PatchValue,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Remove {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub from: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Replace {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub path: Vec<String>,
    pub value: PatchValue,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Move {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub from: Vec<String>,
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub path: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Copy {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub from: Vec<String>,
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub path: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Test {
    #[serde(deserialize_with = "deserialize_slash_separated")]
    pub path: Vec<String>,
    pub value: PatchValue,
}

#[cfg(test)]
mod success {
    use super::*;

    #[test]
    fn example1() {
        let json = r#"{
            "op":"add",
            "path":"/foo/bar",
            "value":"bar2"
        }"#;
        let result: Result<JsonPatchOp, _> = serde_json::from_str(json);
        let expected = Add {
            path: vec!["".to_string(), "foo".to_string(), "bar".to_string()],
            value: PatchValue::String("bar2".to_string()),
        };
        assert_eq!(result.unwrap(), JsonPatchOp::Add(expected));
    }
}
