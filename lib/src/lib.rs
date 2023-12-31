pub mod models;
use std::{collections::HashMap, str::FromStr};

use ethers_core::{
    abi::Address,
    types::{
        BigEndianHash, Block, Bytes, Chain, GethDebugBuiltInTracerType, GethDebugTracerType,
        GethDebugTracingOptions, GethTrace, GethTraceFrame, Log, PreStateFrame, Transaction, H256,
        U256,
    },
    utils::{hex, keccak256},
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
        primitives::{ruint::Uint, AccountInfo, B256},
        Database,
    },
    utils::{h160_to_b160, u256_to_ru256},
};
use regex::Regex;

use models::MethodType;

pub async fn run(
    chainid: u64,
    etherscan_api_key: String,
    contract_address: String,
    tx_hash: String,
    endpoint: String,
    method_type: MethodType,
) -> eyre::Result<Vec<String>> {
    eprintln!("run started");
    let chain = Chain::try_from(chainid).unwrap();
    let mut contract_metadata =
        get_source_from_etherscan(chain, contract_address.clone(), etherscan_api_key).await?;
    eprintln!("Got contracts from etherscan");

    let patched_metadata = patch_metadata_source(&mut contract_metadata).await?;
    eprintln!("Patched source code");

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

    eprintln!("Compiled source code");

    let produced_logs =
        simulate_tx(endpoint, tx_hash, contract_address, bytecode, method_type).await?;

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
    method_type: MethodType,
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

    let env = evm_opts.evm_env().await.unwrap();
    let db = Backend::spawn(evm_opts.get_fork(&config, env.clone())).await;
    let builder = ExecutorBuilder::default()
        .with_config(env)
        .with_spec(evm_spec(&config.evm_version))
        .with_cheatcodes(CheatsConfig::new(&config, &evm_opts));

    let mut executor = builder.build(db);

    let mut env = configure_env_for_executor(&executor, tx.block_number.unwrap().as_u64(), &block);

    match method_type {
        MethodType::Plain => {
            prepare_fork_state_plain(
                &mut executor,
                &mut env,
                &block,
                tx.transaction_index.unwrap().as_u64(),
            )
            .unwrap();
        }
        MethodType::Prestate => {
            prepare_fork_state_debug(&mut executor, &provider, tx.hash.clone())
                .await
                .unwrap();
        }
    }

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

fn prepare_fork_state_plain(
    executor: &mut Executor,
    mut env: &mut Env,
    block: &Block<Transaction>,
    tx_index: u64,
) -> Result<()> {
    for (_, replayed_tx) in block.transactions.iter().enumerate() {
        if replayed_tx
            .transaction_index
            .unwrap()
            .as_u64()
            .eq(&tx_index)
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

    Ok(())
}

async fn prepare_fork_state_debug(
    executor: &mut Executor,
    provider: &foundry_common::RetryProvider,
    tx_hash: H256,
) -> Result<()> {
    let states = provider
        .debug_trace_transaction(
            tx_hash,
            GethDebugTracingOptions {
                tracer: Some(GethDebugTracerType::BuiltInTracer(
                    GethDebugBuiltInTracerType::PreStateTracer,
                )),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let test = match states {
        GethTrace::Known(GethTraceFrame::PreStateTracer(x)) => x,
        _ => panic!("Unknown trace type"),
    };

    apply_pre_state(executor.backend_mut(), test).unwrap();
    Ok(())
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

pub fn apply_pre_state(db: &mut Backend, pre_state: PreStateFrame) -> DatabaseResult<()> {
    let prestate_mode = match pre_state {
        PreStateFrame::Default(prestate_mode) => prestate_mode.to_owned(),
        _ => panic!("Unsupported PreStateFrame"),
    };

    for (account, account_overrides) in prestate_mode.0 {
        // let mut account_info = db.basic((account).into())?.unwrap_or_default();
        let mut account_info = AccountInfo::default();

        if let Some(nonce) = account_overrides.nonce {
            // convert to nonce to U256 to u64
            account_info.nonce = nonce.as_u64();
        }
        if let Some(code) = account_overrides.code {
            account_info.code = Some(Bytecode::new_raw(hex::decode(&code[2..]).unwrap().into()));
        }
        if let Some(balance) = account_overrides.balance {
            account_info.balance = balance.into();
        }

        db.insert_account_info(account, account_info);

        if let Some(storage) = account_overrides.storage {
            for (key, value) in storage.iter() {
                db.active_fork_db_mut().unwrap().insert_account_storage(
                    (account).into(),
                    key.into_uint().into(),
                    value.into_uint().into(),
                )?;
            }
        }
    }
    Ok(())
}

fn print_logs(logs: &[Log]) {
    for log in decode_console_logs(logs) {
        eprint!("{:?}\n", log);
    }
}
