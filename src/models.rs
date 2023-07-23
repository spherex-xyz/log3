use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Log3Json {
    pub chainid: u64,
    pub etherscan_api_key: String,
    pub contract_address: String,
    pub tx_hash: String,
    pub endpoint: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Log3Res {
    pub log_lines: Vec<String>,
}
