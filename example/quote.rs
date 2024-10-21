use alloy::primitives::{address, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use node_db::NodeDB;
use revm::wiring::default::TransactTo;
use revm::wiring::EthereumWiring;
use revm::Evm;

// Balance of function signature
sol!(
    #[sol(rpc)]
    contract Quote {
        function getAmountsOut(uint amountIn, address[] memory path) public view returns (uint[] memory amounts);
    }
);

#[tokio::main]

async fn main() -> Result<()> {
    // on-chain addresses
    let vitalik = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
    let uniswap = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");

    // setup the db
    let database_path = String::from("/mnt/eth-docker-data");
    let mut nodedb = NodeDB::new(database_path).unwrap();

    // construct the calldata for weth->usdc quote
    let out_call = Quote::getAmountsOutCall {
        amountIn: U256::from(1e16),
        path: vec![weth, usdc],
    }
    .abi_encode();

    // create evm instance and transact
    let mut evm = Evm::<EthereumWiring<&mut NodeDB, ()>>::builder()
        .with_db(&mut nodedb)
        .with_default_ext_ctx()
        .modify_cfg_env(|env| {
            env.disable_nonce_check = true;
        })
        .modify_tx_env(|tx| {
            tx.caller = vitalik;
            tx.value = U256::ZERO;
            tx.transact_to = TransactTo::Call(uniswap);
            tx.data = out_call.into();
        })
        .build();

    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;
    let output = result.output().unwrap();
    let decoded_outputs = <Vec<U256>>::abi_decode(output, false).unwrap();
    println!(
        "Swapped 1 WETH for {} USDC",
        decoded_outputs.get(1).unwrap()
    );

    Ok(())
}
