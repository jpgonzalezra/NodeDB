use std::time::Instant;

use alloy_primitives::{address, U256};
use alloy_sol_types::{SolCall, SolValue, sol};
use eyre::anyhow;
use eyre::Result;
use node_db::NodeDB;
use revm::context::result::ExecutionResult;
use revm::primitives::{Address, TxKind};
use revm::{Context, DatabaseRef, ExecuteEvm, MainBuilder, MainContext};

sol! {
    #[sol(rpc)]
    contract WETH {
        function balanceOf(address account) external view returns (uint256);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    // on chain addresses
    let account = address!("0000000000000000000000000000000000000001");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

    // construct the database
    let database_path = std::env::var("DB_PATH").unwrap().parse().unwrap();
    let mut nodedb = NodeDB::new(database_path).unwrap();

    let start = Instant::now();
    let balance_slot = get_balance_slot(&mut nodedb, weth, account)?;
    let duration = start.elapsed();

    println!(
        "Resolved WETH balance slot: 0x{:x} (in {:?})",
        balance_slot, duration
    );

    Ok(())
}

fn get_balance_slot(nodedb: &mut NodeDB, token: Address, account: Address) -> Result<U256> {
    nodedb.enable_tracing()?;

    let balanceof_calldata = WETH::balanceOfCall { account }.abi_encode();
    let mut evm = Context::mainnet()
        .with_db(&mut *nodedb)
        .modify_tx_chained(|tx| {
            tx.caller = address!("0000000000000000000000000000000000000000");
            tx.kind = TxKind::Call(token);
            tx.data = balanceof_calldata.into();
        })
        .build_mainnet();
    let ref_tx = evm.replay()?;

    let balance = match ref_tx.result {
        ExecutionResult::Success { output, .. } => U256::abi_decode(output.data(), false)?,
        result => return Err(anyhow!("balanceOf failed: {result:?}")),
    };

    let accessed_slots = nodedb.get_accessed_slots(token)?;
    nodedb.disable_tracing();

    // Identify the storage slot whose value matches the expected balance
    for slot in &accessed_slots {
        let slot_value = nodedb.storage_ref(token, *slot)?;
        if slot_value == balance {
            return Ok(*slot);
        }
    }

    Err(anyhow!("Balance slot not found among accessed slots"))
}
