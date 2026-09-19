use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseData<T> {
    #[serde(deserialize_with = "deserialize_code")]
    pub code: i32,
    pub message: Option<String>,
    pub data: Option<T>,
}

impl<T> ResponseData<T> {
    pub fn is_success(&self) -> bool {
        self.code == 1
    }
}

fn deserialize_code<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct CodeVisitor;

    impl<'de> Visitor<'de> for CodeVisitor {
        type Value = i32;

        #[cfg_attr(coverage, coverage(off))]
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an integer or string code")
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
            Ok(v as i32)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v as i32)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            v.parse().map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(CodeVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_success_when_code_one() {
        let resp: ResponseData<()> = serde_json::from_str(r#"{"code":1,"message":"ok"}"#).unwrap();
        assert!(resp.is_success());
    }

    #[test]
    fn deserialize_code_from_string() {
        let resp: ResponseData<()> =
            serde_json::from_str(r#"{"code":"1","message":"ok"}"#).unwrap();
        assert_eq!(resp.code, 1);
    }

    #[test]
    fn deserialize_code_from_i64() {
        let resp: ResponseData<()> = serde_json::from_str(r#"{"code":-1}"#).unwrap();
        assert_eq!(resp.code, -1);
    }
}
