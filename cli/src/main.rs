use clap::Parser;
use log3_lib::{self, models::MethodType};

#[derive(Parser)]
#[command(name = "Log3")]
struct Cli {
    /// Chain to use (Default=1)
    #[arg(short, long)]
    chainid: Option<u64>,

    /// Explorer API Key
    etherscan_api_key: String,

    /// Contract Address
    contract_address: String,

    /// TX Hash
    tx_hash: String,

    /// RPC Endpoint
    endpoint: String,

    /// Method to use (0-normal fork, 1-debug prestate (default) )
    #[arg(short, long)]
    method_type: Option<u8>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let chainid = match cli.chainid {
        Some(chainid) => chainid,
        _ => 1,
    };

    let method_type: MethodType = match cli.method_type {
        Some(method_type) => MethodType::from(method_type),
        _ => MethodType::Prestate,
    };

    let run_rs = log3_lib::run(
        chainid,
        cli.etherscan_api_key,
        cli.contract_address,
        cli.tx_hash,
        cli.endpoint,
        method_type,
    )
    .await?;

    for v in run_rs {
        println!("{}", v);
    }

    Ok(())
}
