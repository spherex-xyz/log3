use serde::{Deserialize, Serialize};
use serde_repr::*;

#[derive(Clone, Debug, PartialEq, Default, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum MethodType {
    Plain = 0,
    #[default]
    Prestate = 1,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Log3Json {
    pub chainid: u64,
    pub etherscan_api_key: String,
    pub contract_address: String,
    pub tx_hash: String,
    pub endpoint: String,
    pub method: Option<MethodType>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Log3Res {
    pub log_lines: Vec<String>,
}
