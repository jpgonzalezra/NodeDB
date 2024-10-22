use alloy::primitives::{address, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use node_db::{InsertionType, NodeDB};
use revm::state::{AccountInfo, Bytecode};
use revm::wiring::default::TransactTo;
use revm::wiring::EthereumWiring;
use revm::Evm;

// Generate contract bindings
sol!(Counter, "example/counter.json");

#[tokio::main]
async fn main() -> Result<()> {
    // dummy addresses
    let counter_address = address!("A5C381211A406b48A073E954e6949B0D49506bc0");
    let caller = address!("0000000000000000000000000000000000000001");

    // construct the database
    let database_path = String::from("/mnt/eth-docker-data");
    let mut nodedb = NodeDB::new(database_path).unwrap();

    // insert contract account
    let counter_bytecode = Bytecode::new_raw(Counter::DEPLOYED_BYTECODE.clone());
    let counter_bytecode_hash = counter_bytecode.hash_slow();
    let counter_account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0_u64,
        code: Some(counter_bytecode),
        code_hash: counter_bytecode_hash,
    };
    nodedb.insert_account_info(counter_address, counter_account, InsertionType::Custom);

    let increment_calldata = Counter::incrementCall {}.abi_encode();

    // construct the evm instance
    let mut evm = Evm::<EthereumWiring<&mut NodeDB, ()>>::builder()
        .with_db(&mut nodedb)
        .with_default_ext_ctx()
        .modify_cfg_env(|env| {
            env.disable_nonce_check = true;
        })
        .modify_tx_env(|tx| {
            tx.caller = caller;
            tx.transact_to = TransactTo::Call(counter_address);
            tx.data = increment_calldata.into();
            tx.value = U256::ZERO;
        })
        .build();

    // transact and commit this transaction to the database!
    evm.transact_commit().unwrap();

    let getcount_calldata = Counter::getCountCall {}.abi_encode();
    evm.tx_mut().data = getcount_calldata.into();

    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;
    let output = result.output().unwrap();
    let decoded_output = <U256>::abi_decode(output, false).unwrap();
    println!("Counter after increment = {}", decoded_output);

    Ok(())
}
