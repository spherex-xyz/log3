use std::{collections::HashMap, str::FromStr};

use ethers_core::{
    abi::Address,
    types::{BigEndianHash, Block, Bytes, Chain, Log, Transaction, H256, U256},
    utils::keccak256,
};
use ethers_etherscan::{
    contract::{Metadata, SourceCodeEntry, SourceCodeMetadata},
    Client,
};
use ethers_providers::Middleware;
use ethers_solc::EvmVersion;
use eyre::Result;
use foundry_common::compile::compile_from_source;
use foundry_config::Config;
use foundry_evm::{
    decode::decode_console_logs,
    executor::{
        backend::{DatabaseError, DatabaseResult},
        inspector::{cheatcodes::util::configure_tx_env, CheatsConfig},
        opts::EvmOpts,
        Backend, Bytecode, DeployResult, Env, Executor, ExecutorBuilder, RawCallResult, SpecId,
    },
    revm::{
        primitives::{ruint::Uint, B256},
        Database,
    },
    utils::{h160_to_b160, u256_to_ru256},
};
use regex::Regex;

pub async fn run(
    chainid: u64,
    etherscan_api_key: String,
    contract_address: String,
    tx_hash: String,
    endpoint: String,
) -> eyre::Result<Vec<String>> {
    let chain = Chain::try_from(chainid).unwrap();
    let mut contract_metadata =
        get_source_from_etherscan(chain, contract_address.clone(), etherscan_api_key).await?;

    let patched_metadata = patch_metadata_source(&mut contract_metadata).await?;

    let (_, contract_bytecode) = compile_from_source(&patched_metadata).await?;
    let bytecode = contract_bytecode
        .clone()
        .deployed_bytecode
        .bytecode
        .unwrap()
        .object
        .as_bytes()
        .unwrap()
        .clone();

    let produced_logs = simulate_tx(endpoint, tx_hash, contract_address, bytecode).await?;

    Ok(produced_logs)
}

async fn get_source_from_etherscan(
    chain: Chain,
    contract_address: String,
    etherscan_api_key: String,
) -> Result<Metadata> {
    let client = Client::new(chain, etherscan_api_key)?;
    let metadata = client
        .contract_source_code(contract_address.parse()?)
        .await?;

    let compile_metadata = metadata.items.first().unwrap();

    Ok(compile_metadata.clone())
}

async fn add_console_to_source(source_code: &mut String) -> Result<()> {
    source_code.insert_str(0, include_str!("console.sol"));
    Ok(())
}

async fn add_console_to_sources(sources: &mut HashMap<String, SourceCodeEntry>) -> Result<()> {
    sources.insert(
        String::from("hardhat/console.sol"),
        SourceCodeEntry {
            content: include_str!("console.sol").to_string(),
        },
    );

    Ok(())
}

async fn patch_metadata_source(metadata: &Metadata) -> Result<Metadata> {
    let mut patched_metadata = metadata.clone();

    match (&mut patched_metadata.source_code, &metadata.source_code) {
        (
            SourceCodeMetadata::SourceCode(source_code),
            SourceCodeMetadata::SourceCode(orig_source_code),
        ) => {
            source_code.clear();
            source_code.push_str(patch_source_unit(&orig_source_code).await.as_str());
            add_console_to_source(source_code).await?;
        }
        (
            SourceCodeMetadata::Metadata {
                language: _,
                sources,
                settings: _,
            },
            SourceCodeMetadata::Metadata {
                language: _,
                sources: orig_sources,
                settings: _,
            },
        ) => {
            sources.clear();
            for (source_path, source_entry) in orig_sources.iter() {
                let content = format!(
                    "import \"hardhat/console.sol\";\n\n{}",
                    patch_source_unit(&source_entry.content).await
                );
                sources.insert(source_path.clone(), SourceCodeEntry { content });
            }
            add_console_to_sources(sources).await?;
        }
        _ => {}
    };

    Ok(patched_metadata)
}

async fn patch_source_unit(source_unit: &String) -> String {
    let re = Regex::new(r"(\n[ \t]*)//([ \t]*console.log)").unwrap();
    let patched_source_unit = re.replace_all(&source_unit, "$1$2");

    patched_source_unit.to_string()
}

async fn simulate_tx(
    endpoint: String,
    tx_hash: String,
    contract_address: String,
    code: Bytes,
) -> Result<Vec<String>> {
    let figment = Config::figment().merge(("eth_rpc_url", endpoint.clone()));
    let mut evm_opts = figment.extract::<EvmOpts>().unwrap();
    let config = Config::from_provider(figment).sanitized();
    let provider = get_provider(&config);

    let mut tx = provider
        .get_transaction(H256::from_str(tx_hash.as_str()).unwrap())
        .await?
        .unwrap();

    let block = provider
        .get_block_with_txs(tx.block_hash.unwrap())
        .await?
        .unwrap();

    evm_opts.fork_url = Some(config.get_rpc_url_or_localhost_http().unwrap().into_owned());
    evm_opts.fork_block_number = Some(tx.block_number.unwrap().as_u64() - 1);

    let env = evm_opts.evm_env().await;
    let db = Backend::spawn(evm_opts.get_fork(&config, env.clone())).await;
    let builder = ExecutorBuilder::default()
        .with_config(env)
        .with_spec(evm_spec(&config.evm_version))
        .with_cheatcodes(CheatsConfig::new(&config, &evm_opts));

    let mut executor = builder.build(db);

    let mut env = configure_env_for_executor(&executor, tx.block_number.unwrap().as_u64(), &block);

    for (_, replayed_tx) in block.transactions.into_iter().enumerate() {
        if replayed_tx
            .transaction_index
            .unwrap()
            .as_u64()
            .eq(&tx.transaction_index.unwrap().as_u64())
        {
            break;
        }

        configure_tx_env(&mut env, &replayed_tx);

        if let Some(_) = replayed_tx.to {
            // trace!(tx=?tx.hash,?to, "executing previous call transaction");
            executor.commit_tx_with_env(env.clone()).unwrap();
        } else {
            // trace!(tx=?tx.hash, "executing previous create transaction");
            executor.deploy_with_env(env.clone(), None).unwrap();
        }
    }

    // let code2=Bytes::from_str("0x608060405234801561001057600080fd5b50600436106100415760003560e01c8063371303c01461004657806361bc221a146100505780636d4ce63c1461006e575b600080fd5b61004e61008c565b005b6100586100c9565b60405161006591906101f8565b60405180910390f35b6100766100cf565b60405161008391906101f8565b60405180910390f35b60008081548092919061009e90610242565b91905055506100c76040518060600160405280602c815260200161034b602c913960005461011a565b565b60005481565b60006101126040518060400160405280602081526020017f5b436f6e736f6c65546573745d5b6765745d20636f756e74657220697320256481525060005461011a565b600054905090565b6101b2828260405160240161013092919061031a565b6040516020818303038152906040527fb60e72cc000000000000000000000000000000000000000000000000000000007bffffffffffffffffffffffffffffffffffffffffffffffffffffffff19166020820180517bffffffffffffffffffffffffffffffffffffffffffffffffffffffff83818316178352505050506101b6565b5050565b60008151905060006a636f6e736f6c652e6c6f679050602083016000808483855afa5050505050565b6000819050919050565b6101f2816101df565b82525050565b600060208201905061020d60008301846101e9565b92915050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601160045260246000fd5b600061024d826101df565b91507fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff820361027f5761027e610213565b5b600182019050919050565b600081519050919050565b600082825260208201905092915050565b60005b838110156102c45780820151818401526020810190506102a9565b60008484015250505050565b6000601f19601f8301169050919050565b60006102ec8261028a565b6102f68185610295565b93506103068185602086016102a6565b61030f816102d0565b840191505092915050565b6000604082019050818103600083015261033481856102e1565b905061034360208301846101e9565b939250505056fe5b436f6e736f6c65546573745d5b696e635d20636f756e74657220696e6372656d656e74656420746f202564a26469706673582212206fa42d8d11c48ed1600ee9e9380519c792400117c51cc826e5d10de84e6ea44c64736f6c63430008120033").unwrap();

    let overrides = StateOverride::from([(
        Address::from_str(contract_address.as_str()).unwrap(),
        AccountOverride {
            code: Some(code.clone()),
            ..Default::default()
        },
    )]);

    let _ = apply_state_override(executor.backend_mut(), overrides.clone()).unwrap();

    let result = {
        executor
            .set_tracing(true)
            // .set_debugger(true)
            .set_trace_printer(false);
        tx.gas_price = Some(U256::from(1)); //tx.gas_price * 0.000001;
                                            // tx.gas = tx.gas * 2000;
        tx.max_priority_fee_per_gas = Some(U256::from(1));
        tx.max_fee_per_gas = Some(U256::from(1));
        configure_tx_env(&mut env, &tx);
        env.tx.gas_limit *= 2000;

        // let mut run_result: RunResult = RunResult {
        //     // original_gas_used: receipt.gas_used.unwrap().as_u64(),
        //     tx_hash: tx.hash,
        //     // original_gas_limit: tx.gas.as_u64(),
        //     ..Default::default()
        // };

        let logs = if let Some(_) = tx.to {
            // trace!(tx=?tx.hash,to=?to, "executing call transaction");
            let RawCallResult {
                reverted: _,
                gas_used: _,
                traces: _,
                logs,
                // debug: _debug,
                exit_reason: _,
                ..
            } = executor.commit_tx_with_env(env).unwrap();
            logs
        } else {
            // trace!(tx=?tx.hash, "executing create transaction");
            let DeployResult {
                gas_used: _,
                logs,
                traces: _,
                // debug: run_debug,
                ..
            }: DeployResult = executor.deploy_with_env(env, None).unwrap();
            logs
        };
        logs
    };

    print_logs(&result);

    Ok(decode_console_logs(&result))
}

fn get_provider(config: &Config) -> foundry_common::RetryProvider {
    let url = config.get_rpc_url_or_localhost_http().unwrap();
    let chain = config.chain_id.unwrap_or_default();
    foundry_common::ProviderBuilder::new(url.as_ref())
        .chain(chain)
        .build()
        .unwrap()
}

fn evm_spec(evm: &EvmVersion) -> SpecId {
    match evm {
        EvmVersion::Istanbul => SpecId::ISTANBUL,
        EvmVersion::Berlin => SpecId::BERLIN,
        EvmVersion::London => SpecId::LONDON,
        EvmVersion::Paris => SpecId::MERGE,
        _ => panic!("Unsupported EVM version"),
    }
}

fn configure_env_for_executor(
    executor: &Executor,
    tx_block_number: u64,
    block: &Block<Transaction>,
) -> Env {
    let mut env = executor.env().clone();
    env.block.number = Uint::from(tx_block_number);

    env.block.timestamp = block.timestamp.into();
    env.block.coinbase = block.author.unwrap_or_default().into();
    env.block.difficulty = block.difficulty.into();
    env.block.prevrandao = match block.mix_hash {
        None => None,
        Some(x) => Option::Some(x.into()),
    };
    env.block.basefee = Uint::from(1); //WARN SIMULATING //block.base_fee_per_gas.unwrap_or_default().into();
    let gas_limit = block.gas_limit.as_u64() * 2000;
    env.block.gas_limit = Uint::from(gas_limit);

    return env;
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct AccountOverride {
    pub nonce: Option<u64>,
    pub code: Option<Bytes>,
    pub balance: Option<U256>,
    pub state: Option<HashMap<H256, H256>>,
    pub state_diff: Option<HashMap<H256, H256>>,
}

type StateOverride = HashMap<Address, AccountOverride>;

fn apply_state_override(db: &mut Backend, overrides: StateOverride) -> DatabaseResult<()> {
    for (account, account_overrides) in overrides.iter() {
        let mut account_info = db.basic(h160_to_b160(*account))?.unwrap_or_default();

        if let Some(nonce) = account_overrides.nonce {
            account_info.nonce = nonce;
        }
        if let Some(code) = &account_overrides.code {
            account_info.code_hash = B256::from_slice(&keccak256(code.as_ref())[..]);
            account_info.code = Some(Bytecode::new_raw(code.to_vec().into()));
        }
        if let Some(balance) = account_overrides.balance {
            account_info.balance = balance.into();
        }

        db.insert_account_info(*account, account_info);

        // We ensure that not both state and state_diff are set.
        // If state is set, we must mark the account as "NewlyCreated", so that the old storage
        // isn't read from
        match (&account_overrides.state, &account_overrides.state_diff) {
            (Some(_), Some(_)) => {
                return Err(DatabaseError::msg(
                    "state and state_diff can't be used together".to_string(),
                ))
            }
            (None, None) => (),
            (Some(new_account_state), None) => {
                db.active_fork_db_mut().unwrap().replace_account_storage(
                    h160_to_b160(*account),
                    new_account_state
                        .iter()
                        .map(|(key, value)| {
                            (
                                u256_to_ru256(key.into_uint()),
                                u256_to_ru256(value.into_uint()),
                            )
                        })
                        .collect(),
                )?;
            }
            (None, Some(account_state_diff)) => {
                for (key, value) in account_state_diff.iter() {
                    db.insert_account_storage(
                        *account,
                        key.into_uint().into(),
                        value.into_uint().into(),
                    )?;
                }
            }
        };
    }
    Ok(())
}

fn print_logs(logs: &[Log]) {
    for log in decode_console_logs(logs) {
        print!("{:?}\n", log);
    }
}
