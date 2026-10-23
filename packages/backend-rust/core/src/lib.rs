//! Business operations, independent of transport and serialization.
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Property {
    pub name: Vec<u16>,
    pub kind: Option<String>,
}
pub struct Endpoint {
    pub name: Vec<u16>,
    pub properties: Vec<Property>,
}
pub struct Catalog(Vec<Endpoint>);
impl Catalog {
    pub fn new(endpoints: Vec<Endpoint>) -> Self {
        Self(endpoints)
    }
    pub fn names(&self) -> Vec<Vec<u16>> {
        self.0
            .iter()
            .map(|endpoint| endpoint.name.clone())
            .collect()
    }
    pub fn lookup(&self, name: &[u16]) -> Option<Vec<(Vec<u16>, String)>> {
        self.0
            .iter()
            .find(|endpoint| endpoint.name == name)
            .map(|endpoint| {
                endpoint
                    .properties
                    .iter()
                    .map(|property| {
                        let kind = match &property.kind {
                            None => "string".to_owned(),
                            Some(kind) => {
                                let mut kind = kind.clone();
                                if let Some(first) = kind.get_mut(..1) {
                                    first.make_ascii_uppercase();
                                }
                                kind
                            }
                        };
                        (property.name.clone(), kind)
                    })
                    .collect()
            })
    }
}

pub fn ping_at(now: SystemTime) -> Result<i64, &'static str> {
    let millis = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_millis()).map_err(|_| "invalid clock")?,
        Err(error) => -i128::try_from(error.duration().as_millis()).map_err(|_| "invalid clock")?,
    };
    if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&millis) {
        return Err("invalid clock");
    }
    Ok(millis as i64)
}
pub fn echo(params_json: &str) -> &str {
    params_json
}
